/* ---- settings (⌘,) ----
 *
 * One section, and it is the one `ACTIONS` was always shaped to feed: what
 * this window answers to. Orca keeps ~70 remappable actions behind the same
 * chord; ours are defaults only so far, which the panel says outright rather
 * than letting a person hunt for an edit affordance that is not there. */

const settingsView = el("settings-view");

/* ---- the section rail ----
 *
 * Orca's settings draws ONE section at a time: the rail holds the sections
 * (`SettingsSidebar`) and `activeSectionId` picks the pane. Ours holds only
 * the sections this window actually has, in Orca's own groups. */
/* Which pane a named control lives on, for the two rows that open settings
 * pointed at a control rather than a screen. */
const PANE_OF = {
  "terminal-command": "terminal",
  "settings-keys": "shortcuts",
  "account-list": "provider-accounts",
  "codex-account-list": "provider-accounts",
  "google-account-list": "provider-accounts",
  "router-provider-list": "api-routers",
  "router-preset-select": "api-routers",
  // The Jev dashboard's "in settings" and its consent chip land on the one
  // switch (§6.1), its key chip on the key field and its budget chip on the
  // key card's status (t-6243 D5).
  "jev-enabled": "api-routers",
  "typesafe-key-input": "api-routers",
  "typesafe-status": "api-routers",
  "show-automations": "appearance",
  "show-tasks": "appearance",
  "worktree-prefix": "git",
  "source-control-group-order": "git",
  "source-control-compare-base": "git",
  "refresh-local-base-ref-on-worktree-create": "git",
  "settings-open-in-apps": "general",
  "browser-terminal-link-actions": "browser",
  // The composer's gear lands here — Orca's "Agents — manage AI agents, set a
  // default" is the same screen, reached from the same place.
  "agent-pills": "agents",
};
const settingsRailItems = [...settingsView.querySelectorAll(".settings-rail-item")];
/* Remembered across closes, the way Orca keeps `activeSectionId`. */
let settingsPane = "general";

/* ---- one canonical settings document ------------------------------------
 *
 * Every persisted preference comes back as the same versioned document. A
 * control may paint the person's intent immediately, but only this function
 * accepts what is durable. That matters in two ordinary cases: a backend can
 * clamp an answer, and another window can commit a newer answer while this
 * one's older request is still in flight.
 *
 * Revision is the ordering fact. A response from an older commit is ignored;
 * equal revisions are deliberately re-applied so a failed optimistic write
 * can roll back by re-reading the unchanged canonical document. */
let settingsRevision = -1;
let settingsHydrated = false;
let settingsReadGeneration = 0;
const settingsMutationGenerations = new Map();
let settingsMutationEpoch = 0;
let settingsMutationsInFlight = 0;
let settingsRefreshDeferred = false;
let settingsHealth = "healthy";
let settingsHealthError = null;
let secondBrainVault = "";
/* 주간 리뷰를 zo cron 턴으로 — 카드의 스위치. 기록 자체는 볼트의 zo 등록부에
 * 있고 영수증은 상태 답에 실려 온다. */
let secondBrainWeeklyReview = false;
const SETTINGS_HEALTH_RANK = { healthy: 0, migrated: 0, recovered: 1, error: 2 };

const hasSetting = (snapshot, key) =>
  snapshot !== null && typeof snapshot === "object" &&
  Object.prototype.hasOwnProperty.call(snapshot, key);

function sameSetting(left, right) {
  if (Object.is(left, right)) return true;
  if (Array.isArray(left) || Array.isArray(right)) {
    return Array.isArray(left) && Array.isArray(right)
      && left.length === right.length
      && left.every((value, index) => Object.is(value, right[index]));
  }
  if (
    left === null || right === null ||
    typeof left !== "object" || typeof right !== "object"
  ) return false;
  const leftKeys = Object.keys(left);
  const rightKeys = Object.keys(right);
  return leftKeys.length === rightKeys.length
    && leftKeys.every((key) =>
      Object.prototype.hasOwnProperty.call(right, key) && Object.is(left[key], right[key]));
}

/* Storage health is part of the canonical snapshot, not a toast inferred from
 * whichever read happened most recently. Keeping the backend's safe message
 * as text also means an error can describe a path without becoming markup. */
function paintSettingsHealth() {
  const banner = el("settings-health");
  const title = el("settings-health-title");
  const detail = el("settings-health-detail");
  const visible = settingsHealth === "recovered" || settingsHealth === "error";
  banner.hidden = !visible;
  banner.dataset.health = settingsHealth;
  if (!visible) return;

  const failed = settingsHealth === "error";
  banner.setAttribute("role", failed ? "alert" : "status");
  banner.setAttribute("aria-live", failed ? "assertive" : "polite");
  if (failed) {
    say(title, () => t("settings.storage.errorTitle", "설정을 불러오지 못했습니다"));
    say(detail, () => settingsHealthError || t("settings.storage.errorFallback", "이번 실행에서는 대체 설정을 사용합니다."));
    return;
  }
  say(title, () => t("settings.storage.recoveredTitle", "설정을 복구했습니다"));
  say(detail, () => t("settings.storage.recoveredBody", "손상된 설정 파일을 가장 최근의 정상 백업에서 복구했습니다. 설정값을 확인하세요."));
}

function applyAppearanceSettingsSnapshot(snapshot, first) {
  const previousSystemLocale = systemLocale;
  let repaintLeftSidebar = false;
  let repaintUiZoom = false;
  if (hasSetting(snapshot, "left_sidebar_appearance_spec")) {
    leftSidebarAppearanceSpec = snapshot.left_sidebar_appearance_spec ?? null;
    repaintLeftSidebar = true;
  }
  if (hasSetting(snapshot, "system_locale")) systemLocale = snapshot.system_locale ?? "ko";
  if (
    hasSetting(snapshot, "locale") &&
    (first || snapshot.locale !== locale || previousSystemLocale !== systemLocale)
  ) {
    setLocale(snapshot.locale ?? "system", { refresh: !first, persist: false });
  }
  if (hasSetting(snapshot, "theme") && (first || snapshot.theme !== theme)) {
    theme = snapshot.theme ?? "system";
    applyTheme();
    paintThemePicker();
  }
  if (hasSetting(snapshot, "ui_zoom_spec")) {
    uiZoomSpec = snapshot.ui_zoom_spec ?? null;
    repaintUiZoom = true;
  }
  if (hasSetting(snapshot, "ui_zoom_level")) {
    uiZoomLevel = normalizedUiZoomLevel(snapshot.ui_zoom_level);
    repaintUiZoom = true;
  }
  if (repaintUiZoom) paintUiZoom();
  if (hasSetting(snapshot, "app_font_family")) {
    appFontFamily = snapshot.app_font_family?.trim() ?? "";
    paintAppFontFamily();
  }
  if (hasSetting(snapshot, "show_titlebar_app_name")) {
    showTitlebarAppName = snapshot.show_titlebar_app_name !== false;
    paintTitlebarAppName();
  }
  if (hasSetting(snapshot, "show_menu_bar_icon")) {
    showMenuBarIcon = snapshot.show_menu_bar_icon !== false;
    paintMenuBarIconPreference();
  }
  if (hasSetting(snapshot, "minimize_to_tray_on_close")) {
    minimizeToTrayOnClose = snapshot.minimize_to_tray_on_close === true;
    paintMinimizeToTrayPreference();
  }
  if (hasSetting(snapshot, "compact_worktree_cards")) {
    compactWorktreeCards = snapshot.compact_worktree_cards === true;
    paintWorktreeCardLayout();
  }
  if (hasSetting(snapshot, "show_git_ignored_files")) {
    const next = snapshot.show_git_ignored_files !== false;
    const changed = next !== showGitIgnoredFiles;
    showGitIgnoredFiles = next;
    paintGitIgnoredFilesPreference();
    if (!first && changed) loadTree(fileTree, "").catch(showError);
  }
  if (hasSetting(snapshot, "left_sidebar_appearance_mode")) {
    leftSidebarAppearanceMode = normalizedLeftSidebarAppearanceMode(
      snapshot.left_sidebar_appearance_mode,
    );
    repaintLeftSidebar = true;
  }
  if (hasSetting(snapshot, "left_sidebar_tint_color")) {
    leftSidebarTintColor = normalizedLeftSidebarTintColor(snapshot.left_sidebar_tint_color);
    repaintLeftSidebar = true;
  }
  if (hasSetting(snapshot, "left_sidebar_tint_opacity")) {
    leftSidebarTintOpacity = normalizedLeftSidebarTintOpacity(
      snapshot.left_sidebar_tint_opacity,
    );
    repaintLeftSidebar = true;
  }
  if (repaintLeftSidebar) paintLeftSidebarAppearance();
  let repaintStatusBar = false;
  if (hasSetting(snapshot, "usage_percentage_display")) {
    usagePercentageDisplay = normalizedUsagePercentageDisplay(snapshot.usage_percentage_display);
    repaintStatusBar = true;
  }
  if (hasSetting(snapshot, "status_bar_usage_mode")) {
    statusBarUsageMode = normalizedStatusBarUsageMode(snapshot.status_bar_usage_mode);
    repaintStatusBar = true;
  }
  if (hasSetting(snapshot, "usage_analytics_off")) {
    const off = new Set(snapshot.usage_analytics_off ?? []);
    const same = off.size === usageAnalyticsOff.size
      && [...off].every((one) => usageAnalyticsOff.has(one));
    if (!same) {
      usageAnalyticsOff.clear();
      for (const one of off) usageAnalyticsOff.add(one);
      paintUsageStats();
    }
  }
  if (hasSetting(snapshot, "source_control_view_mode")) {
    const mode = normalizedScmViewMode(snapshot.source_control_view_mode);
    if (mode !== scmViewMode) {
      scmViewMode = mode;
      scmFoldedDirs.clear();
      scmTreeRows.clear();
      paintScm();
    }
  }
  if (hasSetting(snapshot, "status_bar_items")) {
    statusBarItems = normalizedStatusBarItems(snapshot.status_bar_items);
    repaintStatusBar = true;
  }
  if (repaintStatusBar) paintStatusBarPreferences();
}

function applyEditingSettingsSnapshot(snapshot, first) {
  if (hasSetting(snapshot, "editing_prefs_spec")) {
    editingPrefsSpec = snapshot.editing_prefs_spec ?? editingPrefsSpec;
  }
  if (
    hasSetting(snapshot, "editing_prefs") &&
    (first || !sameSetting(snapshot.editing_prefs, editingPrefs))
  ) {
    applyEditingPrefs(snapshot.editing_prefs);
  }
  if (hasSetting(snapshot, "diff_side_by_side")) {
    const changed = diffSideBySide !== (snapshot.diff_side_by_side !== false);
    diffSideBySide = snapshot.diff_side_by_side !== false;
    paintEditingPrefs();
    if (!first && changed) {
      for (const held of tabs) if (held.kind === "diff") paintDiffView(held);
    }
  }
}

function applyTerminalSettingsSnapshot(snapshot, first) {
  if (hasSetting(snapshot, "terminal_prefs_spec")) {
    termPrefsSpec = snapshot.terminal_prefs_spec ?? termPrefsSpec;
  }
  if (
    hasSetting(snapshot, "terminal_prefs") &&
    (first || !sameSetting(snapshot.terminal_prefs, termPrefs))
  ) {
    const before = termPrefs;
    termPrefs = { ...(termPrefs ?? {}), ...(snapshot.terminal_prefs ?? {}) };
    applyTermPrefs(before);
    paintTermPrefs();
  }
  if (first && usesCommandModifier) void refreshTerminalKeyboardLayout();
  if (first && usesWindowsPlatform && !isPopout) {
    void refreshTerminalWindowsStatus();
  }
  if (hasSetting(snapshot, "terminal_command")) {
    terminalCommand = snapshot.terminal_command ?? "";
    terminalCommandField.value = terminalCommand;
  }
  if (hasSetting(snapshot, "setup_script_launch_mode")) {
    setupScriptLaunchMode = normalizedSetupScriptLaunchMode(
      snapshot.setup_script_launch_mode,
    );
    paintSetupScriptLaunchMode();
  }
  const hasOpacity = hasSetting(snapshot, "terminal_opacity");
  if (hasOpacity) {
    terminalOpacity = snapshot.terminal_opacity ?? OPACITY_DEFAULT;
  }
  const hasBlur = hasSetting(snapshot, "window_blur");
  if (hasBlur) windowBlur = snapshot.window_blur === true;
  const hasBootBlur = hasSetting(snapshot, "window_blur_active");
  if (hasBootBlur) {
    blurAtBoot = snapshot.window_blur_active === true;
  }
  if (hasOpacity || hasBlur || hasBootBlur) {
    applyWindowMaterial();
    paintWindowControls();
  }
}

function applyAgentSettingsSnapshot(snapshot) {
  if (hasSetting(snapshot, "default_agent")) {
    defaultAgent = normalizeDefaultAgent(snapshot.default_agent);
    paintAgentPills();
  }
  if (hasSetting(snapshot, "agent_teams_mode")) {
    agentTeamsMode = snapshot.agent_teams_mode ?? "panes";
    el("orch-teams-mode").value = agentTeamsMode;
  }
  if (hasSetting(snapshot, "claude_autoswitch_mode")) {
    claudeAutoSwitchMode = snapshot.claude_autoswitch_mode ?? "ask";
    el("account-autoswitch").value = claudeAutoSwitchMode;
  }
  // 대화의 Focus view(확장 2.1.221): 이 창의 모든 대화가 입는 한 값이라,
  // 권위 있는 스냅샷이 올 때마다 서 있는 목록들이 그 값을 따라간다.
  if (hasSetting(snapshot, "conversation_focus_view")) {
    conversationFocusView = snapshot.conversation_focus_view === true;
    repaintFocusView();
  }
}

function applyNavigationSettingsSnapshot(snapshot) {
  if (hasSetting(snapshot, "terminal_shortcut_policy")) {
    terminalShortcutPolicy = normalizeTerminalShortcutPolicy(
      snapshot.terminal_shortcut_policy,
    );
    paintTerminalShortcutPolicy();
  }
  if (hasSetting(snapshot, "ctrl_tab_order_mode")) {
    ctrlTabOrderMode = normalizeCtrlTabOrderMode(snapshot.ctrl_tab_order_mode);
    cancelTabSwitcher();
    paintCtrlTabOrderMode();
  }
  if (hasSetting(snapshot, "panel_widths")) {
    for (const side of ["sidebar", "aside"]) {
      const width = snapshot.panel_widths?.[side];
      if (typeof width === "number") panelWidths[side] = width;
      applyPanelWidth(side);
      el(side === "sidebar" ? "grip-sidebar" : "grip-aside").setAttribute(
        "aria-valuenow",
        String(panelWidths[side]),
      );
    }
  }
  if (hasSetting(snapshot, "hidden_shortcuts")) {
    hiddenShortcuts.clear();
    for (const name of snapshot.hidden_shortcuts ?? []) hiddenShortcuts.add(name);
    paintShortcuts();
  }
  if (hasSetting(snapshot, "keybindings")) {
    keybindingOverrides = snapshot.keybindings ?? {};
    rebuildBound();
    paintSettingsKeys();
    paintChordTitles();
  }
}

function applyFloatingWorkspaceSettingsSnapshot(snapshot) {
  if (!hasSetting(snapshot, "floating_workspace")) return;
  const held = snapshot.floating_workspace;
  floatingWorkspacePrefs = {
    enabled: held?.enabled !== false,
    cwd: typeof held?.cwd === "string" && held.cwd.trim() !== "" ? held.cwd : "~",
    trigger_location: normalizedFloatingTriggerLocation(held?.trigger_location),
  };
  paintFloatingWorkspacePane();
  paintFloatingWorkspaceTriggers();
  // A flag-off takes the panel down — Orca's own effect
  // (use-floating-workspace-panel.ts:124-131). Its hydration gate is not
  // needed here: at boot the panel is closed anyway, and closing a closed
  // panel is nothing.
  if (!floatingWorkspacePrefs.enabled && !termFloat.hidden) closeFloatingPanelKeepingFocus();
}

function applyWorkflowSettingsSnapshot(snapshot, first) {
  if (hasSetting(snapshot, "vault_limits")) vaultLimits = snapshot.vault_limits;
  if (hasSetting(snapshot, "vault.sessionLimit")) {
    const changed = vaultSessionLimit !== snapshot["vault.sessionLimit"];
    vaultSessionLimit = snapshot["vault.sessionLimit"];
    if (changed) {
      if (vaultDockAnswer || vaultDockLoading) void refreshVaultDock();
      if (vaultAnswer || vaultLoading) void refreshVault();
    }
  }
  const hasDefaultTaskSource = hasSetting(snapshot, "default_task_source");
  if (hasDefaultTaskSource) {
    defaultTaskSource = snapshot.default_task_source ?? "jira";
  }
  const hasHiddenTaskSources = hasSetting(snapshot, "hidden_task_sources");
  if (hasHiddenTaskSources) {
    hiddenTaskSources.clear();
    for (const name of snapshot.hidden_task_sources ?? []) hiddenTaskSources.add(name);
  }
  if (hasDefaultTaskSource || hasHiddenTaskSources) {
    if (
      first
      || hiddenTaskSources.has(chosenTaskSource)
      || !SUPPORTED_TASK_SOURCES.has(chosenTaskSource)
    ) {
      chosenTaskSource = defaultTaskSource;
    }
    paintTaskSources();
    paintDefaultTaskSource();
  }

  if (hasSetting(snapshot, "opencode_cookie_configured")) {
    const changed = opencodeCookieConfigured !== (snapshot.opencode_cookie_configured === true);
    opencodeCookieConfigured = snapshot.opencode_cookie_configured === true;
    // The cookie itself is never in a snapshot — it lives in the keychain.
    // The field shows a placeholder for "one is on file" and empties when the
    // person takes it back, so the box never claims to hold what it cannot.
    el("opencode-cookie").value = "";
    el("opencode-cookie").placeholder = opencodeCookieConfigured
      ? t("settings.opencode.cookieHeld", "저장됨 — 새 값을 붙여넣으면 바뀝니다")
      : "auth=Fe26.2**…";
    if (!first && changed) {
      void refreshProviderUsage(usageProvider("opencode-go"), opencodeCookieConfigured);
    }
  }
  if (hasSetting(snapshot, "opencode_workspace")) {
    el("opencode-workspace").value = snapshot.opencode_workspace ?? "";
  }
  if (hasSetting(snapshot, "second_brain_vault")) {
    secondBrainVault = String(snapshot.second_brain_vault ?? "").trim();
    paintKnowledgeEntry();
  }
  // 지식 그래프의 저장 시야·검색어(볼트별 한 줄) — 부팅 보고와 다른 창의 저장이
  // 이 문으로 들어온다. 그래프는 이 표에서만 읽는다(`noteKnowledgeScenes`).
  if (hasSetting(snapshot, "second_brain_scenes")) {
    noteKnowledgeScenes(snapshot.second_brain_scenes);
  }
  // 지식 그래프의 마지막 탐색 자리(t-4140 S2, 볼트별 한 줄) — 같은 문, 같은 손.
  if (hasSetting(snapshot, "second_brain_explore")) {
    noteKnowledgeExploreLines(snapshot.second_brain_explore);
  }
  if (hasSetting(snapshot, "second_brain_weekly_review")) {
    secondBrainWeeklyReview = snapshot.second_brain_weekly_review === true;
    paintSecondBrainWeeklyReview();
  }
  if (hasSetting(snapshot, "hide_automation_workspaces")) {
    const changed = hideAutomationWorkspaces !== (snapshot.hide_automation_workspaces === true);
    hideAutomationWorkspaces = snapshot.hide_automation_workspaces === true;
    if (!first && changed) void refreshWorktrees();
  }
  if (hasSetting(snapshot, "hide_default_branch_workspaces")) {
    const changed =
      hideDefaultBranchWorkspaces !== (snapshot.hide_default_branch_workspaces === true);
    hideDefaultBranchWorkspaces = snapshot.hide_default_branch_workspaces === true;
    if (!first && changed) void refreshWorktrees();
  }
  if (hasSetting(snapshot, "hide_detached_head_workspaces")) {
    const changed =
      hideDetachedHeadWorkspaces !== (snapshot.hide_detached_head_workspaces === true);
    hideDetachedHeadWorkspaces = snapshot.hide_detached_head_workspaces === true;
    if (!first && changed) void refreshWorktrees();
  }
  if (hasSetting(snapshot, "hide_sleeping_workspaces")) {
    const changed = hideSleepingWorkspaces !== (snapshot.hide_sleeping_workspaces === true);
    hideSleepingWorkspaces = snapshot.hide_sleeping_workspaces === true;
    if (!first && changed) void refreshWorktrees();
  }
  if (hasSetting(snapshot, "keep_default_branch_awake")) {
    // 없음은 켬이다 — 그것이 이 기본값의 요점이다.
    const changed = keepDefaultBranchAwake !== (snapshot.keep_default_branch_awake !== false);
    keepDefaultBranchAwake = snapshot.keep_default_branch_awake !== false;
    if (!first && changed) void refreshWorktrees();
  }
  if (hasSetting(snapshot, "hide_agent_scratch_workspaces")) {
    const changed =
      hideAgentScratchWorkspaces !== (snapshot.hide_agent_scratch_workspaces === true);
    hideAgentScratchWorkspaces = snapshot.hide_agent_scratch_workspaces === true;
    if (!first && changed) void refreshWorktrees();
  }
  if (hasSetting(snapshot, "worktree_card_properties")) {
    worktreeCardProperties.clear();
    for (const property of snapshot.worktree_card_properties ?? []) {
      worktreeCardProperties.add(property);
    }
    paintWorktreeCardProperties();
  }
  if (hasSetting(snapshot, "agent_activity_display")) {
    agentActivityDisplay = snapshot.agent_activity_display === "full" ? "full" : "compact";
    paintWorktreeAgents();
  }
  if (hasSetting(snapshot, "workspace_board")) {
    const board = snapshot.workspace_board ?? {};
    const statuses = Array.isArray(board.statuses) && board.statuses.length > 0
      ? board.statuses.map((status) => ({
          id: String(status.id ?? ""),
          label: String(status.label ?? ""),
          color: String(status.color ?? "neutral"),
          icon: String(status.icon ?? "circle-dot"),
        }))
      : DEFAULT_WORKSPACE_BOARD_STATUSES.map((status) => ({ ...status }));
    const cards = board.cards && typeof board.cards === "object" && !Array.isArray(board.cards)
      ? Object.fromEntries(
          Object.entries(board.cards).map(([path, card]) => [path, {
            status: typeof card?.status === "string" ? card.status : null,
            pinned: card?.pinned === true,
          }]),
        )
      : {};
    workspaceBoardSettings = {
      statuses,
      cards,
      column_width: Math.max(220, Math.min(520, Number(board.column_width) || 308)),
    };
    if (workspaceBoardOpen) {
      paintWorkspaceBoard();
      if (!el("workspace-board-settings-pop").hidden) paintWorkspaceBoardSettings();
    }
  }
  if (hasSetting(snapshot, "sidebar_view")) {
    const view = snapshot.sidebar_view ?? {};
    const changed = sidebarGroupBy !== (view.group_by ?? "repo")
      || sidebarSortBy !== (view.sort_by ?? "default")
      || sidebarProjectOrder !== (view.project_order ?? "default");
    sidebarGroupBy = view.group_by ?? "repo";
    sidebarSortBy = view.sort_by ?? "default";
    sidebarProjectOrder = view.project_order ?? "default";
    if (!first && changed) void refreshWorktrees();
  }
  if (hasSetting(snapshot, "confirm_close_pinned")) {
    confirmClosePinnedTab = snapshot.confirm_close_pinned !== false;
    paintPinnedConfirm();
  }
  for (const kind of COMPUTER_CONFIRM_KINDS) {
    if (hasSetting(snapshot, `computer_confirm_${kind}`)) {
      computerConfirmPolicy[kind] = snapshot[`computer_confirm_${kind}`] !== false;
      paintComputerConfirm();
    }
  }
  if (hasSetting(snapshot, "skip_close_terminal_with_running_process_confirm")) {
    const skip = snapshot.skip_close_terminal_with_running_process_confirm === true;
    setConfirmCloseRunning(!skip);
    if (skip) {
      closeRunningLegacyMigrationStarted = true;
      forgetLegacyCloseRunningPreference();
    } else if (first && legacySkipCloseRunningConfirm && !closeRunningLegacyMigrationStarted) {
      // Pre-canonical builds stored the inverse in this webview only. Carry it
      // once into the repository; a failed write leaves the old key for the
      // next boot and the canonical recovery snapshot turns prompting back on.
      closeRunningLegacyMigrationStarted = true;
      setConfirmCloseRunning(false);
      void commitSetting(
        "skip_close_terminal_with_running_process_confirm",
        "set_skip_close_terminal_with_running_process_confirm",
        { skip: true },
      ).then((saved) => {
        if (saved?.skip_close_terminal_with_running_process_confirm === true) {
          forgetLegacyCloseRunningPreference();
        }
      });
    }
  }
  if (hasSetting(snapshot, "workspace_creation_prefs")) {
    workspaceCreationPrefs = {
      directory: snapshot.workspace_creation_prefs?.directory ?? "~/zerocode/workspaces",
      nest_workspaces: snapshot.workspace_creation_prefs?.nest_workspaces !== false,
    };
    paintWorkspaceCreationPrefs();
  }
  const hasOpenInApplicationsSpec = hasSetting(snapshot, "open_in_applications_spec");
  if (hasOpenInApplicationsSpec) {
    openInApplicationsSpec = {
      max: snapshot.open_in_applications_spec?.max ?? 0,
      presets: snapshot.open_in_applications_spec?.presets ?? [],
    };
  }
  const hasOpenInApplications = hasSetting(snapshot, "open_in_applications");
  if (hasOpenInApplications) {
    openInApplications = (snapshot.open_in_applications ?? []).map((application) => ({
      id: String(application.id ?? ""),
      label: String(application.label ?? ""),
      command: String(application.command ?? ""),
    }));
    openInApplicationDraft = null;
  }
  if (hasOpenInApplicationsSpec || hasOpenInApplications) paintOpenInApplications();
  if (hasSetting(snapshot, "skip_delete_worktree_confirm")) {
    skipDeleteWorktreeConfirm = snapshot.skip_delete_worktree_confirm === true;
    paintDeleteWorktreeConfirm();
  }
  if (hasSetting(snapshot, "skip_delete_automation_confirm")) {
    skipDeleteAutomationConfirm = snapshot.skip_delete_automation_confirm === true;
    paintDeleteAutomationConfirm();
  }
  if (hasSetting(snapshot, "artifacts_retention_spec")) {
    artifactsRetentionSpec = snapshot.artifacts_retention_spec ?? artifactsRetentionSpec;
  }
  if (hasSetting(snapshot, "artifacts_retention_days")) {
    artifactsRetentionDays = Number(snapshot.artifacts_retention_days) || 0;
    paintArtifactsRetention();
  }
  if (hasSetting(snapshot, "computer_awake_mode")) {
    computerAwakeMode = normalizeComputerAwakeMode(snapshot.computer_awake_mode);
    paintComputerAwakeMode();
  }
  if (hasSetting(snapshot, "worktree_prefs")) {
    worktreePrefs = snapshot.worktree_prefs ?? null;
    paintWorktreePrefs();
  }
  if (hasSetting(snapshot, "source_control_group_order")) {
    sourceControlGroupOrder = normalizeSourceControlGroupOrder(
      snapshot.source_control_group_order,
    );
    paintSourceControlGroupOrderPreference();
    paintSourceControlGroupOrder();
  }
  if (hasSetting(snapshot, "source_control_compare_base")) {
    const next = normalizeSourceControlCompareBase(snapshot.source_control_compare_base);
    const changed = sourceControlCompareBase !== next;
    sourceControlCompareBase = next;
    paintSourceControlCompareBasePreference();
    if (!first && changed && scmPanelShowing()) void refreshSourceControlCompare();
  }
  if (hasSetting(snapshot, "refresh_local_base_ref_on_worktree_create")) {
    refreshLocalBaseRefOnWorktreeCreate =
      snapshot.refresh_local_base_ref_on_worktree_create === true;
    paintRefreshLocalBaseRefPreference();
  }
  if (hasSetting(snapshot, "notifications")) {
    notificationPrefs = { ...notificationPrefs, ...(snapshot.notifications ?? {}) };
    paintNotificationPrefs();
  }
  const hasBrowserZoomSpec = hasSetting(snapshot, "browser_zoom_spec");
  if (hasBrowserZoomSpec) {
    browserZoomSpec = { ...(snapshot.browser_zoom_spec ?? {}) };
  }
  const hasBrowserPrefs = hasSetting(snapshot, "browser");
  if (hasBrowserPrefs) {
    browserPrefs = { ...browserPrefs, ...(snapshot.browser ?? {}) };
    // The visit ledger, back from disk — once, at boot. Later snapshots echo
    // what this window itself saved, and an echo must not roll a livelier
    // in-memory count back.
    if (browserVisits.size === 0) {
      for (const visit of browserPrefs.visits ?? []) {
        if (typeof visit?.url === "string" && visit.url !== "") {
          browserVisits.set(visit.url, {
            title: visit.title ?? "",
            count: visit.count ?? 1,
            at: visit.at ?? 0,
          });
        }
      }
    }
  }
  if (hasBrowserZoomSpec || hasBrowserPrefs) {
    paintBrowserPrefs();
  }
}

function applySettingsSnapshot(snapshot) {
  if (snapshot === null || typeof snapshot !== "object") return false;
  const revision = Number.isInteger(snapshot.revision) ? snapshot.revision : null;
  if (hasSetting(snapshot, "settings_health") || hasSetting(snapshot, "settings_error")) {
    const reportedHealth = typeof snapshot.settings_health === "string"
      ? snapshot.settings_health
      : typeof snapshot.settings_error === "string" ? "error" : "healthy";
    const reportedError = typeof snapshot.settings_error === "string"
      ? snapshot.settings_error.trim()
      : null;
    // A healthy re-read proves the current file is readable; it does not make
    // a recovery or boot failure the person has not yet seen stop happening.
    // Keep the worst observation for this renderer session, including the
    // first safe backend detail that explains it.
    if ((SETTINGS_HEALTH_RANK[reportedHealth] ?? 0) > (SETTINGS_HEALTH_RANK[settingsHealth] ?? 0)) {
      settingsHealth = reportedHealth;
      settingsHealthError = reportedError;
    } else if (reportedHealth === "error" && !settingsHealthError && reportedError) {
      settingsHealthError = reportedError;
    }
    paintSettingsHealth();
  }
  // Health is evidence about storage, not versioned preference data. An
  // incompatible or failed recovery snapshot can legitimately carry an older
  // (even zero) revision while its primary remains untouched, so latch that
  // evidence before rejecting stale preference fields.
  if (revision !== null && revision < settingsRevision) return false;

  const first = !settingsHydrated;
  if (hasSetting(snapshot, "explorer_policy")) explorerPolicy = snapshot.explorer_policy;
  // The checks table (t-2733): the poll's interval and the diff cache's caps
  // arrive here and nowhere else, so a changed overlay re-arms the beat.
  if (hasSetting(snapshot, "checks_limits")) {
    checksLimits = snapshot.checks_limits;
    armChecksPoller();
  }
  if (hasSetting(snapshot, "crash_limits")) paintCrashSettings(snapshot.crash_limits);
  applyAppearanceSettingsSnapshot(snapshot, first);
  applyEditingSettingsSnapshot(snapshot, first);
  applyUpdateSettingsSnapshot(snapshot, first);
  applyTerminalSettingsSnapshot(snapshot, first);
  applyAgentSettingsSnapshot(snapshot);
  applyNavigationSettingsSnapshot(snapshot);
  applyWorkflowSettingsSnapshot(snapshot, first);
  applyFloatingWorkspaceSettingsSnapshot(snapshot);

  if (revision !== null) settingsRevision = Math.max(settingsRevision, revision);
  settingsHydrated = true;
  return true;
}

async function fetchSettingsSnapshot({ reportError = false } = {}) {
  try {
    return await invoke("settings_snapshot");
  } catch (error) {
    if (reportError) showError(error);
    return null;
  }
}

function drainDeferredSettingsRefresh() {
  if (!settingsRefreshDeferred || settingsMutationsInFlight !== 0) return;
  settingsRefreshDeferred = false;
  void refreshSettingsSnapshot();
}

function deferSettingsRefresh() {
  settingsRefreshDeferred = true;
  drainDeferredSettingsRefresh();
}

function applySettingsMutationSnapshot(key, generation, mutationEpoch, snapshot) {
  if (snapshot === null) return false;
  if (
    settingsMutationGenerations.get(key) === generation &&
    settingsMutationEpoch === mutationEpoch &&
    settingsMutationsInFlight === 1
  ) {
    return applySettingsSnapshot(snapshot);
  }
  // A mutation answer is a full document. If any other optimistic mutation
  // overlaps it, even one that started earlier, this answer cannot own every
  // field; leave the optimistic values standing and converge once all writes
  // have settled.
  deferSettingsRefresh();
  return false;
}

async function refreshSettingsSnapshot(options = {}) {
  const generation = ++settingsReadGeneration;
  const mutationEpoch = settingsMutationEpoch;
  const safeAtStart = settingsMutationsInFlight === 0;
  const snapshot = await fetchSettingsSnapshot(options);
  if (snapshot !== null && generation === settingsReadGeneration) {
    if (
      safeAtStart &&
      settingsMutationsInFlight === 0 &&
      settingsMutationEpoch === mutationEpoch
    ) {
      applySettingsSnapshot(snapshot);
    } else {
      // A full document read across optimistic work cannot own the screen.
      // Remember one canonical read instead; the last mutation to settle (or
      // this read itself, if the mutation already settled) drains that bit.
      deferSettingsRefresh();
    }
  }
  return snapshot;
}

/* All setting writes share the same recovery rule. A rejection and a lost
 * acknowledgement are indistinguishable to the webview, so both re-read the
 * authoritative document before reporting the failure. */
async function commitSetting(key, command, args, { onError = null } = {}) {
  const generation = (settingsMutationGenerations.get(key) ?? 0) + 1;
  settingsMutationGenerations.set(key, generation);
  const mutationEpoch = ++settingsMutationEpoch;
  settingsMutationsInFlight += 1;
  try {
    const snapshot = await invoke(command, args);
    applySettingsMutationSnapshot(key, generation, mutationEpoch, snapshot);
    return snapshot;
  } catch (error) {
    // A newer write for this SAME field owns both the visible value and any
    // error beside it. An older rejection arriving now is history: refetching
    // for it can roll back optimistic UI, and reporting it can paint an error
    // over the newer value that already saved successfully.
    if (settingsMutationGenerations.get(key) !== generation) return undefined;
    const snapshot = await fetchSettingsSnapshot();
    // The recovery read crosses an async boundary. A newer write for this
    // field changes its generation; a write for ANY field changes the epoch.
    // In either case this full document began life before newer optimistic
    // state and is no longer allowed to repaint it.
    applySettingsMutationSnapshot(key, generation, mutationEpoch, snapshot);
    if (settingsMutationGenerations.get(key) !== generation) return undefined;
    onError?.(error);
    showError(error);
    return undefined;
  } finally {
    settingsMutationsInFlight -= 1;
    drainDeferredSettingsRefresh();
  }
}

el("ssh-hosts-head").addEventListener("click", () => {
  sshHostsFolded = !sshHostsFolded;
  paintSidebarSshHosts();
});

listen("ssh:link-changed", (event) => {
  const report = event.payload;
  if (typeof report?.id !== "string") return;
  sshLinkStates.set(report.id, report);
  paintSshLinkSurfaces();
});

/* ---- SSH가 비밀을 물을 때 (P0-20) ----
 *
 * Orca의 런타임 자격증명 프롬프트(ssh-passphrase.ts) 이식: 비밀이 모자란
 * 연결이 ssh:credential-request로 이 창에 묻고 120초를 기다린다. 대답은
 * ssh_submit_credential 하나로 돌아가고 — 값, 또는 거절의 null — 어느
 * 쪽으로 끝났든 ssh:credential-resolved가 그 질문의 모달을 닫는다(우리가
 * 답했든, 저쪽에서 시간이 다했든 같은 문). 질문이 겹치면 줄을 선다:
 * 모달은 한 번에 한 질문만 들고, 앞이 끝나야 다음이 오른다. 비밀은 제출
 * 순간 입력칸에서 지워지고 어디에도 남지 않는다. */
const sshCredScrim = el("ssh-cred-scrim");
const sshCredValue = el("ssh-cred-value");
let sshCredCurrent = null;
const sshCredQueue = [];

function paintSshCredential(request) {
  sshCredCurrent = request.requestId;
  const passphrase = request.kind === "passphrase";
  el("ssh-cred-title").textContent = passphrase
    ? t("ssh.credential.passphraseTitle", "SSH 키 암호")
    : t("ssh.credential.passwordTitle", "SSH 비밀번호");
  el("ssh-cred-label").textContent = passphrase
    ? t("ssh.credential.passphraseLabel", "키 암호")
    : t("ssh.credential.passwordLabel", "비밀번호");
  el("ssh-cred-detail").textContent = request.detail ?? "";
  sshCredValue.value = "";
  showModal(sshCredScrim);
}

function showNextSshCredential() {
  if (sshCredCurrent !== null) return;
  const next = sshCredQueue.shift();
  if (next) paintSshCredential(next);
}

function answerSshCredential(value) {
  if (sshCredCurrent === null) return;
  const requestId = sshCredCurrent;
  sshCredCurrent = null;
  sshCredValue.value = "";
  hideModal(sshCredScrim);
  void invoke("ssh_submit_credential", { requestId, value });
  showNextSshCredential();
}

listen("ssh:credential-request", (event) => {
  const request = event.payload;
  if (typeof request?.requestId !== "string") return;
  if (sshCredCurrent === null) paintSshCredential(request);
  else sshCredQueue.push(request);
});

listen("ssh:credential-resolved", (event) => {
  const requestId = event.payload?.requestId;
  // 줄에 서 있던 질문이 저쪽에서 먼저 끝났으면(시간 초과) 줄에서 뺀다.
  const queued = sshCredQueue.findIndex((one) => one.requestId === requestId);
  if (queued >= 0) {
    sshCredQueue.splice(queued, 1);
    return;
  }
  if (requestId !== sshCredCurrent) return;
  sshCredCurrent = null;
  sshCredValue.value = "";
  hideModal(sshCredScrim);
  showNextSshCredential();
});

el("ssh-cred-submit").addEventListener("click", () => {
  answerSshCredential(sshCredValue.value);
});
el("ssh-cred-cancel").addEventListener("click", () => {
  answerSshCredential(null);
});
sshCredValue.addEventListener("keydown", (event) => {
  if (event.key === "Enter") {
    event.preventDefault();
    answerSshCredential(sshCredValue.value);
  } else if (event.key === "Escape") {
    event.preventDefault();
    answerSshCredential(null);
  }
});

listen("settings:changed", (event) => {
  const revision = event.payload?.revision;
  if (Number.isInteger(revision) && revision <= settingsRevision) return;
  if (event.payload?.keys?.includes("sshHosts")
      && settingsPane === "ssh-hosts" && !settingsView.hidden) {
    void refreshSshHosts();
  }
  if (event.payload?.keys?.includes("remoteWorkspaces")
      && settingsPane === "ssh-hosts" && !settingsView.hidden) {
    void refreshRemoteWorkspaces();
  }
  if (event.payload?.keys?.includes("remoteServers")
      && settingsPane === "remote-servers" && !settingsView.hidden) {
    void refreshRemoteServers();
  }
  if (event.payload?.keys?.includes("sshTargets")) {
    if (settingsPane === "ssh" && !settingsView.hidden) void refreshSshTargets();
    void refreshSidebarSshHosts();
  }
  void refreshSettingsSnapshot();
}).catch(() => {});

listen("settings:open", () => {
  if (!isPopout) setSettingsOpen(true);
}).catch(() => {});

/* What each rail item can be found by. The words a person types are the ones
 * they READ, so a section's own name and the labels of the controls on it are
 * what the query is matched against — Orca's `searchEntries` per section, with
 * the same effect and none of its per-control registry. */
function paneWords(pane) {
  const item = settingsRailItems.find((held) => held.dataset.pane === pane);
  const section = settingsView.querySelector(`.settings-pane[data-pane="${pane}"]`);
  // The page's own text, plus the words it is looked up by that it does not
  // say. A control that became a menu took its rows off the page with it, and
  // a pane nobody can find by the name of the thing it reports is a pane that
  // is not there. Orca keeps a hand-written keyword catalogue per pane for the
  // same reason; ours is only what the rendered text cannot supply.
  const keywords = section?.dataset.keywords ?? "";
  return `${item?.textContent ?? ""} ${section?.textContent ?? ""} ${keywords}`.toLowerCase();
}

function paneMatches(pane, query) {
  return query === "" || paneWords(pane).includes(query);
}

function settingsPaneAvailable(pane) {
  return pane !== "macos-permissions" || usesCommandModifier;
}

function showSettingsPane(pane) {
  const arriving = settingsView.hidden || pane !== settingsPane;
  settingsPane = pane;
  // Twelve directory walks; asked for when the page is opened rather than at
  // boot, and only the first time — the row carries a 다시 읽기 for the rest.
  if (pane === "skills" || pane === "onboarding") void refreshSkills();
  // The same on-arrival rule: eleven directory walks and a PATH read, spent
  // when somebody looks, once — 다시 확인 carries the rest.
  if (pane === "orchestration" && orchReport === null && !orchLoading) {
    void refreshOrchestration();
  }
  if (pane === "computer-use" && arriving) {
    void refreshComputerUse();
  }
  // 프로필은 다른 창이 만든 것도 이 목록의 것이라, 올 때마다 파일을 다시
  // 읽는다 — 읽기 하나 값이고 이 페이지는 자주 열리지 않는다.
  if (pane === "browser") void refreshBrowserProfiles().then(paintBrowserProfiles);
  if (pane === "stats") paintStatsUsage();
  // The same on-arrival rule the two pages above hold: a first scan reads
  // every transcript on the disk, so it is spent when somebody looks rather
  // than at boot, and only while there is no answer yet — 다시 확인 carries
  // the rest.
  if (pane === "stats") {
    // The counted figures are cheap to ask for and change on their own, so
    // they are re-read on every arrival rather than once.
    void refreshStatsHead();
    paintUsageStats();
    const held = usageStatsProvider === "overview"
      ? USAGE_STATS_LEDGERS.find((one) => usageStatsReports[one.id])
      : usageStatsReports[usageStatsProvider];
    if (!held && !usageStatsScanning) void refreshUsageStats(false);
  }
  if (pane === "provider-accounts" && arriving) {
    void refreshClaudeAccounts();
    void refreshCodexAccounts();
    void refreshGoogleAccount();
    void refreshCliLogins();
  }
  if (pane === "api-routers" && arriving) {
    void refreshApiRouters();
  }
  if (pane === "onboarding" && arriving) {
    paintSettingsOnboarding();
    void refreshSetupGuide().then(paintSettingsOnboarding);
  }
  if (pane === "quick-commands" && arriving) void refreshSettingsQuickCommands();
  // The feed is asked on arrival (the backend judges the policy) and the
  // history read, once per arrival — 「지금 확인」 and 「다시 읽기」 carry the rest.
  if (pane === "update" && arriving) enterUpdatePane();
  if (pane === "terminal" && arriving) void refreshTerminalSessions();
  // The directory field resolves against the disk, and the disk moves while
  // settings is closed — re-asked on arrival, one cheap read.
  if (pane === "floating-workspace" && arriving) paintFloatingWorkspaceCwd();
  // Connection state belongs to the machine, not to this renderer. Re-read it
  // every time the pane is entered so a login or Jira change made in another
  // window is visible when settings is reopened.
  if (pane === "integrations" && arriving) {
    void refreshGithubIntegration();
    void refreshJiraStatus();
    void refreshLinearStatus();
    void refreshGitlabIntegration();
    void refreshSecondBrain();
  }
  if (pane === "ssh-hosts" && arriving) {
    void refreshSshHosts();
    void refreshRemoteWorkspaces();
  }
  if (pane === "ssh" && arriving) void enterSshTargetsPane();
  if (pane === "remote-servers" && arriving) void refreshRemoteServers();
  if (pane === "macos-permissions" && arriving) void refreshDeveloperPermissions();
  const query = el("settings-search").value.trim().toLowerCase();
  // A section that is on screen but does not answer the query would be the
  // rail saying one thing and the page another. Orca resolves it the same
  // way: `SettingsSection` renders only when it is BOTH active and a match.
  const showing = settingsPaneAvailable(pane) && paneMatches(pane, query);
  for (const held of settingsView.querySelectorAll(".settings-pane")) {
    held.hidden = held.dataset.pane !== pane || !showing;
  }
  const nothing = el("settings-nothing");
  nothing.hidden = showing;
  if (!showing) {
    say(nothing, () => t("settings.nothing", "「{{query}}」에 해당하는 설정이 없습니다", {
      query: el("settings-search").value.trim(),
    }));
  }
  for (const item of settingsRailItems) {
    if (item.dataset.pane === pane) item.setAttribute("aria-current", "page");
    else item.removeAttribute("aria-current");
  }
}

settingsRailItems.forEach((item) => {
  item.addEventListener("click", () => showSettingsPane(item.dataset.pane));
});

/* The query narrows the rail, and the rail decides the page.
 *
 * A group whose every item is gone goes with them — a heading over nothing is
 * a heading that lies about what is under it. And if the section on screen
 * fell out of the results, the first one still standing takes over: Orca's
 * `getFallbackVisibleSection`. Left alone, the rail would show one set of
 * names and the page a section absent from it. */
function paintSettingsSearch() {
  const query = el("settings-search").value.trim().toLowerCase();
  const standing = [];
  for (const item of settingsRailItems) {
    const shown = settingsPaneAvailable(item.dataset.pane) && paneMatches(item.dataset.pane, query);
    item.hidden = !shown;
    if (shown) standing.push(item.dataset.pane);
  }
  for (const box of settingsView.querySelectorAll(".settings-rail-group-box")) {
    box.hidden = ![...box.querySelectorAll(".settings-rail-item")].some((item) => !item.hidden);
  }
  showSettingsPane(standing.includes(settingsPane) ? settingsPane : (standing[0] ?? settingsPane));
}

el("settings-search").addEventListener("input", paintSettingsSearch);
el("settings-back").addEventListener("click", () => setSettingsOpen(false));

/* `mod+shift+n` as a person reads it on this operating system. */
function chordLabel(chord) {
  const glyphs = usesCommandModifier
    ? { mod: "⌘", shift: "⇧", alt: "⌥", enter: "↵" }
    : { mod: "Ctrl+", shift: "Shift+", alt: "Alt+", enter: "Enter" };
  return chord
    .split("+")
    .map((part) => glyphs[part] ?? part.toUpperCase())
    .join("");
}

/* Every key this window shows comes through here.
 *
 * `formatKeybindingList`, measured: several bindings read as several bindings
 * joined by `", "`, and an empty list is NOT an empty string — it is a word
 * (`plugin-manifest-Dq3wpxrr.js:6369-6375`, and `ShortcutHintList` renders
 * that same word rather than a blank cell, `Settings-Db2c7R7D.js:3866-3870`).
 *
 * One path exists because five did. A chord written into a string cannot
 * follow a remap, and every place that spelled one — a tooltip, a tab's `＋`,
 * a lane's close button, the message a dead terminal leaves — went on naming
 * a key that had moved. */
function shortcutLabel(actionId) {
  const chords = chordsForId(actionId);
  if (chords.length === 0) return t("settings.keysUnassigned", "지정 안 됨");
  return chords.map(chordLabel).join(", ");
}

/* The same answer where the hint is optional, which Orca distinguishes: its
 * `useOptionalShortcutLabel` returns null and the caller drops the element
 * entirely rather than drawing an empty bracket
 * (`FileExplorer-BNOZ2UAs.js:1458`). */
function optionalShortcutLabel(actionId) {
  const chords = chordsForId(actionId);
  return chords.length === 0 ? null : chords.map(chordLabel).join(", ");
}

/* Every sentence below that names a key asks `optionalShortcutLabel` first
 * and picks one of TWO catalog sentences by the answer — Orca ships both,
 * `"Toggle sidebar"` beside `"Toggle sidebar ({{value0}})"` (`App`,
 * `e4b9e7dff7` / `ce37cf5279`).
 *
 * Written out at each site rather than behind a helper: a helper takes the
 * `t()` call away from the words it translates, and a label whose key and
 * Korean are not inside a `t(` at the point of use is exactly what this
 * window's own gate refuses. */

/* Re-say every tooltip that names a key.
 *
 * Run after `applyLocale`, never instead of it: the catalog owns the words and
 * this owns the key, and doing it in one pass would put the chord back into
 * the translated string where the next remap could not reach it. */
function paintChordTitles() {
  for (const node of document.querySelectorAll("[data-chord]")) {
    const key = node.getAttribute("data-i18n-title");
    if (!key) continue;
    // `i18nSourcedatatip` is what `applyLocale` stores for this attribute —
    // its store key is the target attribute with the non-letters stripped, so
    // moving the target from `title` to `data-tip` moved this name with it.
    const bare = t(key, node.dataset.i18nSourcedatatip || node.dataset.tip);
    const hint = optionalShortcutLabel(node.dataset.chord);
    node.dataset.tip =
      hint === null ? bare : t(`${key}Chord`, `${bare} ({{chord}})`, { chord: hint });
  }
  // The surfaces that compose their own text and therefore cannot be swept.
  // They are called from here, and only from here, so the next one somebody
  // adds is wired by being listed rather than by being remembered.
  paintWorktreeNewTitle();
  paintStagePlaceholder();
  paintFloatToggleState();
  paintSidebarSearchKeys();
  el("wt-chord").textContent = chordLabel("mod+enter");
  say(
    document.querySelector('[data-i18n="settings.keysNote"]'),
    () => t(
      "settings.keysNote",
      "{{modifier}}를 포함한 키로 바꾸거나, 아예 없애거나, 기본값으로 되돌릴 수 있습니다.",
      { modifier: primaryModifierLabel() },
    ),
  );
  refreshChrome();
}

/* The search button owns up to the finder's chord on hover, the way Orca's
 * shows the palette's key combo (SidebarNav.tsx:321-331). No `data-chord`
 * sweep reaches it — the hint is a child span, not the tooltip — so it is
 * listed above like the other composed surfaces. Hidden entirely when the
 * chord was unbound: an empty keycap is a promise with nothing in it. */
function paintSidebarSearchKeys() {
  const keys = el("sidebar-search-keys");
  const hint = optionalShortcutLabel("worktree.jump");
  keys.textContent = hint ?? "";
  keys.hidden = hint === null;
}

/* `＋ 새 워크트리` says one of two things and only one of them names a key, so
 * the sweep above cannot own it. It is repainted by the same call instead —
 * composing it in `refreshWorktrees` alone is exactly how a remap reached
 * every tooltip in the window except this one. */
function paintWorktreeNewTitle() {
  const button = el("worktree-new");
  if (button.disabled) {
    button.dataset.tip = t("worktree.gitOnly", "Git 프로젝트에서만 워크트리를 만들 수 있습니다");
    return;
  }
  const makeChord = optionalShortcutLabel("worktree.create");
  button.dataset.tip = makeChord === null
    ? t("sidebar.newWorktree", "새 워크트리")
    : t("sidebar.newWorktreeChord", "새 워크트리 ({{chord}})", { chord: makeChord });
}

/* Which actions a chord would collide with.
 *
 * Orca reduces a binding to a normalised identity and calls two bindings a
 * conflict when those identities match — `Meta+Control+Alt+Shift+key`, empty
 * string for a modifier that is absent (`plugin-manifest-Dq3wpxrr.js:6290`).
 * `chordOf` already builds ours in one fixed order, so a chord IS its own
 * identity here and no separate normaliser is needed.
 *
 * The digit rule is measured too and is the part that would be missed: for a
 * digit-index action, ONE digit conflicts with all nine, because binding
 * `⌘5` takes a key out of a range that is addressed as a range
 * (`keybindingConflictIdentities`, same file:6305). ⌘1…⌘9 staging a worktree
 * is exactly such a range here. */
function conflictsWith(chord, actionId) {
  const claimed = [];
  const held = BOUND.get(chord);
  if (held && held.id !== actionId) claimed.push(held.title());
  if (digitIndex(chord) !== null) {
    claimed.push(t("settings.laneDigits", "레인 1–9로 이동"));
  }
  return claimed;
}

/* Why a key was refused, under the row it was refused for. Empty text puts
 * the line away — there is nothing to say once the question is answered. */
function sayConflict(row, message) {
  const line = row.querySelector(".keys-conflict");
  line.textContent = message;
  line.hidden = message === "";
}

/* Record the next chord the person presses, and hand it back.
 *
 * Captured on the window while capturing, so the chord under recording never
 * reaches the action it is currently bound to — pressing ⌘W to rebind it must
 * not close the tab on the way. */
let recordingFor = null;

function recordChord(action, row) {
  if (recordingFor) recordingFor.cancel();
  // Whatever this row was refused for last time is answered by trying again.
  sayConflict(row, "");
  const chordCell = row.querySelector(".keys-chord");
  const was = chordCell.textContent;
  chordCell.textContent = t("settings.keysRecording", "키를 누르세요…");
  chordCell.classList.add("is-recording");

  const finish = () => {
    window.removeEventListener("keydown", onKey, true);
    chordCell.classList.remove("is-recording");
    recordingFor = null;
  };
  const onKey = (event) => {
    // A bare modifier is not a chord yet — wait for the key it modifies.
    if (["Meta", "Shift", "Alt", "Control"].includes(event.key)) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.key === "Escape") {
      chordCell.textContent = was;
      finish();
      return;
    }
    if (!hasPrimaryModifier(event)) {
      chordCell.textContent = was;
      finish();
      showError(t(
        "settings.keysNeedsMod",
        "단축키는 {{modifier}}를 포함해야 합니다",
        { modifier: primaryModifierLabel() },
      ));
      return;
    }
    const chord = chordOf(event);
    const clash = conflictsWith(chord, action.id);
    finish();
    if (clash.length > 0) {
      chordCell.textContent = was;
      // On the row, not in the window's error bar. Orca keeps this per action
      // — `${formatKeybindingList([binding])} conflicts with ${labels}.` in a
      // map keyed by action id (`Settings-Db2c7R7D.js:6964`, `:7075`) — and it
      // is the right call: a refusal that scrolls away from the row it is
      // about leaves the list looking like nothing happened.
      sayConflict(row, t("settings.keysConflict", "{{chord}}는 이미 {{holder}}에 쓰입니다.", {
        chord: chordLabel(chord),
        holder: clash.join(", "),
      }));
      return;
    }
    setKeybinding(action.id, [chord]);
  };
  recordingFor = { cancel: () => { chordCell.textContent = was; finish(); } };
  window.addEventListener("keydown", onKey, true);
}

/* One call for all three, the way Orca's own store has it: a list moves the
 * chord, `null` puts the default back, `[]` takes the key away. The backend
 * answers with the whole override map, so this replaces rather than patches. */
async function setKeybinding(actionId, bindings) {
  const answer = await commitSetting("keybindings", "set_keybinding", { actionId, bindings });
  // Compatibility for a renderer fixture and pre-snapshot backend that
  // answered with the override map itself. Current backends include it under
  // `keybindings` in the full snapshot and were already applied above.
  if (
    answer && !hasSetting(answer, "schema_version") && !hasSetting(answer, "keybindings")
  ) {
    keybindingOverrides = answer;
    rebuildBound();
    paintSettingsKeys();
    paintChordTitles();
  }
}

/* Orca files a definition under the SURFACE its key belongs to, not under the
 * namespace of its id: `Global`, `Terminal Panes`, `Tabs`, `Editors`,
 * `File Explorer`, `Agents`, counted out of `KEYBINDING_DEFINITIONS`. Ours
 * carry the same axis, so the screen groups the way its does. */
const KEY_GROUPS = [
  { id: "global", title: () => t("settings.keysGroup.global", "전역") },
  { id: "tabs", title: () => t("settings.keysGroup.tabs", "탭") },
  { id: "panes", title: () => t("settings.keysGroup.panes", "터미널 페인") },
  { id: "editors", title: () => t("settings.keysGroup.editors", "편집기") },
  { id: "agents", title: () => t("settings.keysGroup.agents", "에이전트") },
  { id: "browser", title: () => t("settings.keysGroup.browser", "브라우저") },
];

function groupTitleOf(groupId) {
  return (KEY_GROUPS.find((group) => group.id === groupId) ?? KEY_GROUPS[0]).title();
}

/* Which other actions hold a key this one holds.
 *
 * Scanned across the table rather than read out of `BOUND`, because `BOUND` is
 * keyed BY chord: two actions on one chord collapse to a single entry there,
 * and the one that lost is exactly the row this has to report. A stored file
 * somebody edited by hand is how this arises — the recorder refuses to create
 * one — and a screen that cannot show it is a screen that hides the reason a
 * key does nothing. */
function conflictsForAction(action) {
  const mine = chordsFor(action);
  const clashes = new Set();
  for (const other of ACTIONS) {
    if (other.id === action.id) continue;
    if (chordsFor(other).some((chord) => mine.includes(chord))) clashes.add(other.title());
  }
  if (mine.some((chord) => digitIndex(chord) !== null)) {
    clashes.add(t("settings.laneDigits", "레인 1–9로 이동"));
  }
  return [...clashes];
}

/* Orca's own four, by its own names: `SHORTCUT_FILTER_LABELS = { all,
 * modified, unassigned, conflicts }` (`Settings-Db2c7R7D.js:6385-6390`). */
const KEY_FILTERS = ["all", "modified", "unassigned", "conflicts"];
const KEY_FILTER_LABELS = {
  all: () => t("settings.keysFilterAll", "전체"),
  modified: () => t("settings.keysFilterModified", "변경됨"),
  unassigned: () => t("settings.keysFilterUnassigned", "지정 안 됨"),
  conflicts: () => t("settings.keysFilterConflicts", "충돌"),
};

let keyFilter = "all";
let keyQuery = "";

function keyRowState(action) {
  return {
    all: true,
    modified: action.id in keybindingOverrides,
    unassigned: chordsFor(action).length === 0,
    conflicts: conflictsForAction(action).length > 0,
  };
}

/* What the search looks at, which is Orca's set: the id, the group's title and
 * the formatted chord (`Settings-Db2c7R7D.js:6423-6427`). Its definitions also
 * carry an explicit `searchKeywords` list; ours carry the translated title
 * instead, because that is the word somebody using this window in Korean
 * would actually type. */
function keyRowMatches(action, query) {
  if (query === "") return true;
  const groupTitle = groupTitleOf(action.group);
  return [
    action.id,
    groupTitle,
    action.title(),
    shortcutLabel(action.id),
    ...(action.keywords ?? []),
  ].some((field) => field.toLowerCase().includes(query));
}

function filterCounts() {
  // `+ 1` is the digit row: it is a row this list can draw, so `전체` counts
  // it. The other three do not — it has no action id, so there is no override
  // to have moved, no key to have lost and nothing to collide with.
  const counts = { all: ACTIONS.length + 1, modified: 0, unassigned: 0, conflicts: 0 };
  for (const action of ACTIONS) {
    const state = keyRowState(action);
    for (const name of KEY_FILTERS) {
      if (name !== "all" && state[name]) counts[name] += 1;
    }
  }
  return counts;
}

function paintKeyFilters() {
  const rail = el("keys-filters");
  const counts = filterCounts();
  rail.replaceChildren();
  for (const name of KEY_FILTERS) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = `segment-btn${name === keyFilter ? " is-active" : ""}`;
    button.dataset.filter = name;
    button.setAttribute("role", "tab");
    button.setAttribute("aria-selected", String(name === keyFilter));
    // The count is what makes a filter worth reading before it is pressed:
    // `충돌 0` answers the question without costing a click.
    button.textContent = `${KEY_FILTER_LABELS[name]()} ${counts[name]}`;
    button.addEventListener("click", () => {
      keyFilter = name;
      paintSettingsKeys();
    });
    rail.appendChild(button);
  }
  el("keys-search").placeholder = t("settings.keysSearch", "단축키 검색");
}

function buildKeyRow(action) {
  const row = document.createElement("div");
  row.className = "keys-row";
  row.innerHTML =
    '<span class="keys-title"></span><kbd class="keys-chord"></kbd>' +
    '<button class="keys-edit" type="button"></button>' +
    '<button class="keys-off" type="button" hidden></button>' +
    '<button class="keys-reset" type="button" hidden></button>' +
    '<p class="keys-conflict" role="alert" hidden></p>';
  row.querySelector(".keys-title").textContent = action.title();
  const chords = chordsFor(action);
  // The same formatter every other surface reads from, so the list and the
  // tooltips can never disagree about what a key is.
  const chordCell = row.querySelector(".keys-chord");
  chordCell.textContent = shortcutLabel(action.id);
  // The cell holds a word, not a key, so it stops looking like a key.
  chordCell.classList.toggle("is-unassigned", chords.length === 0);
  const edit = row.querySelector(".keys-edit");
  edit.textContent = t("settings.keysEdit", "바꾸기");
  edit.addEventListener("click", () => recordChord(action, row));
  // The way back only appears once there is something to go back from.
  const reset = row.querySelector(".keys-reset");
  reset.textContent = t("settings.keysReset", "기본값");
  reset.hidden = !(action.id in keybindingOverrides);
  reset.addEventListener("click", () => setKeybinding(action.id, null));
  // The third gesture, and the one with no other way to reach it: Orca's
  // `disableKeybindingAction` sends `[]`, which is not the same instruction
  // as `null` — this action keeps no key at all rather than going back to
  // its default. Offered only while it still has one to lose.
  const off = row.querySelector(".keys-off");
  off.textContent = t("settings.keysDisable", "없애기");
  off.hidden = chords.length === 0;
  off.addEventListener("click", () => setKeybinding(action.id, []));
  // A conflict that is already stored says so without being provoked, the way
  // Orca's `conflictByAction` map does — the row explains why its key is dead.
  const standing = conflictsForAction(action);
  if (standing.length > 0) {
    sayConflict(row, t("settings.keysConflict", "{{chord}}는 이미 {{holder}}에 쓰입니다.", {
      chord: chords.map(chordLabel).join(", "),
      holder: standing.join(", "),
    }));
  }
  return row;
}

function paintSettingsKeys() {
  const list = el("settings-keys");
  list.replaceChildren();
  paintKeyFilters();

  const query = keyQuery.trim().toLowerCase();
  let shown = 0;
  for (const group of KEY_GROUPS) {
    const rows = ACTIONS.filter(
      (action) =>
        action.group === group.id
        && keyRowState(action)[keyFilter]
        && keyRowMatches(action, query),
    );
    // The lane digits are a rule, not nine table entries, so they are said as
    // one line rather than repeated nine times — but they ARE a global
    // shortcut, and drawing them after every group put them under whichever
    // heading happened to come last.
    const digits = group.id === "global" && digitRowMatches(query) ? [digitRow()] : [];
    if (rows.length + digits.length === 0) continue;
    const heading = document.createElement("h4");
    heading.className = "keys-group";
    heading.textContent = group.title();
    list.appendChild(heading);
    for (const action of rows) list.appendChild(buildKeyRow(action));
    for (const row of digits) list.appendChild(row);
    shown += rows.length + digits.length;
  }

  // A list that narrowed to nothing has to say so, or an empty panel reads as
  // a broken one.
  if (shown === 0) {
    const empty = document.createElement("p");
    empty.className = "keys-empty";
    empty.textContent = t("settings.keysNoMatch", "해당하는 단축키가 없습니다.");
    list.appendChild(empty);
  }
}

/* ⌘1…⌘9, as one row. Orca's own definition for this is
 * `workspace.selectByIndex`, and its keywords are copied from there so the
 * range answers a search for `digit` or `1-9` the way its does. */
const DIGIT_KEYWORDS = [
  "shortcut", "global", "workspace", "worktree",
  "select", "switch", "number", "digit", "1-9", "index",
];

function digitRowChord() {
  // Derived from the same formatter as every other row: written out, it was
  // one more string that would go on claiming ⌘ after the range moved.
  return `${chordLabel("mod+1")}…${chordLabel("mod+9")}`;
}

function digitRowMatches(query) {
  // It has no override and no key of its own to lose, so the three narrowing
  // filters do not describe it; only `전체` can show it.
  if (keyFilter !== "all") return false;
  if (query === "") return true;
  return [
    t("settings.laneDigits", "레인 1–9로 이동"),
    digitRowChord(),
    groupTitleOf("global"),
    ...DIGIT_KEYWORDS,
  ].some((field) => field.toLowerCase().includes(query));
}

function digitRow() {
  const row = document.createElement("div");
  row.className = "keys-row";
  row.innerHTML = '<span class="keys-title"></span><kbd class="keys-chord"></kbd>';
  row.querySelector(".keys-title").textContent = t("settings.laneDigits", "레인 1–9로 이동");
  row.querySelector(".keys-chord").textContent = digitRowChord();
  return row;
}


/* ---- the theme ----
 *
 * Both treatments were in ui/tokens.css from the beginning, measured out of
 * Orca's own stylesheet (1-g); what did not exist was any way to reach the
 * light one, so every window ever opened was dark. This is that way.
 *
 * The OS does not drive the window. It answers `시스템 설정`, which is a
 * choice the person made — the token file says so, and the difference shows
 * the moment somebody who wants dark on a light desktop picks `다크` and
 * expects it to stay. */
const themePicker = el("app-theme");

let theme = "system";
let uiZoomSpec = null;
let uiZoomLevel = 0;
let appliedUiZoomLevel = null;
let uiZoomApplyGeneration = 0;
let appFontFamily = "";
let showTitlebarAppName = true;
let showMenuBarIcon = true;
let minimizeToTrayOnClose = false;
let compactWorktreeCards = false;

/* 카드의 에이전트(워커) 행 차림 — 원본의 `agentActivityDisplayMode`
 * (AGENT_ACTIVITY_DISPLAY_OPTIONS, 기본은 원본과 같은 "compact"). 간결은
 * 쉬는 카드를 요약으로 접고(`latestAgentRows`), 전체 목록은 모든 뿌리 행을
 * 언제나 세운다 — 부모별 자식 접기(`agentFolded`)는 두 차림이 같이 쓴다. */
let agentActivityDisplay = "compact";
let leftSidebarAppearanceSpec = null;
let leftSidebarAppearanceMode = "default";
let leftSidebarTintColor = "";
let leftSidebarTintOpacity = 0;

const LEFT_SIDEBAR_APPEARANCE_MODES = new Set(["default", "match-terminal", "tinted"]);
const LEFT_SIDEBAR_SURFACE_PROPERTIES = [
  "--nav-plane",
  "--nav-plane-ink",
  "--nav-plane-accent",
  "--nav-plane-rule",
  "--nav-plane-ring",
  "--left-sidebar-muted-ink",
];

function normalizedUiZoomLevel(level) {
  if (!uiZoomSpec) return 0;
  const numeric = Number(level);
  if (!Number.isFinite(numeric)) return uiZoomSpec.default_level;
  const stepped = Math.round(numeric / uiZoomSpec.step) * uiZoomSpec.step;
  const clamped = Math.min(uiZoomSpec.max_level, Math.max(uiZoomSpec.min_level, stepped));
  return Object.is(clamped, -0) ? 0 : clamped;
}

function uiZoomFactor(level = uiZoomLevel) {
  return uiZoomSpec ? Math.pow(uiZoomSpec.scale_base, normalizedUiZoomLevel(level)) : 1;
}

/* The native webview is the one live zoom consumer. The CSS variable and
 * dataset are observable tokens for app-owned chrome/tests; no CSS transform
 * competes with Tauri's page scale or changes terminal layout a second time. */
function paintUiZoom() {
  if (!uiZoomSpec) return;
  uiZoomLevel = normalizedUiZoomLevel(uiZoomLevel);
  const root = document.documentElement;
  const factor = uiZoomFactor();
  root.dataset.uiZoomLevel = String(uiZoomLevel);
  root.style.setProperty("--ui-zoom-factor", String(factor));
  el("ui-zoom-percent").textContent = `${Math.round(factor * 100)}%`;
  el("ui-zoom-out").disabled = uiZoomLevel <= uiZoomSpec.min_level;
  el("ui-zoom-in").disabled = uiZoomLevel >= uiZoomSpec.max_level;
  el("ui-zoom-reset").disabled = uiZoomLevel === uiZoomSpec.default_level;

  if (appliedUiZoomLevel === uiZoomLevel) return;
  const generation = ++uiZoomApplyGeneration;
  appliedUiZoomLevel = uiZoomLevel;
  invoke("apply_ui_zoom", { zoomLevel: uiZoomLevel }).catch((error) => {
    if (generation === uiZoomApplyGeneration) appliedUiZoomLevel = null;
    showError(error);
  });
}

function setUiZoomLevel(level) {
  if (!uiZoomSpec) return Promise.resolve(null);
  const canonical = normalizedUiZoomLevel(level);
  if (canonical === uiZoomLevel) {
    paintUiZoom();
    return Promise.resolve(null);
  }
  uiZoomLevel = canonical;
  paintUiZoom();
  return commitSetting("ui_zoom_level", "set_ui_zoom", { zoomLevel: canonical });
}

el("ui-zoom-out").addEventListener("click", () => {
  void setUiZoomLevel(uiZoomLevel - uiZoomSpec.step);
});

el("ui-zoom-in").addEventListener("click", () => {
  void setUiZoomLevel(uiZoomLevel + uiZoomSpec.step);
});

el("ui-zoom-reset").addEventListener("click", () => {
  void setUiZoomLevel(uiZoomSpec.default_level);
});

function normalizedLeftSidebarAppearanceMode(mode) {
  return LEFT_SIDEBAR_APPEARANCE_MODES.has(mode) ? mode : "default";
}

function normalizedHexColor(color, fallback) {
  const digits = String(color ?? "").trim().replace(/^#/, "");
  if (!/^(?:[0-9a-f]{3}|[0-9a-f]{6})$/i.test(digits)) return fallback;
  const expanded = digits.length === 3
    ? [...digits].map((glyph) => glyph.repeat(2)).join("")
    : digits;
  return `#${expanded.toLowerCase()}`;
}

function normalizedLeftSidebarTintColor(color) {
  return normalizedHexColor(
    color,
    leftSidebarAppearanceSpec?.tint_color_default ?? "",
  );
}

function normalizedLeftSidebarTintOpacity(opacity) {
  const spec = leftSidebarAppearanceSpec?.tint_opacity;
  if (!spec) return 0;
  const numeric = Number(opacity);
  if (!Number.isFinite(numeric)) return spec.default;
  return Math.min(spec.max, Math.max(spec.min, numeric));
}

function leftSidebarSurface() {
  if (leftSidebarAppearanceMode === "default") return null;
  if (leftSidebarAppearanceMode === "match-terminal") {
    return {
      background: "color-mix(in srgb, var(--term-screen-bg) calc(var(--term-screen-alpha, 1) * 100%), transparent)",
      foreground: "var(--term-screen-fg)",
    };
  }
  const strength = Number((leftSidebarTintOpacity * 100).toFixed(2));
  return {
    background: `color-mix(in srgb, ${leftSidebarTintColor} ${strength}%, var(--surface-abyss))`,
    foreground: "var(--ink-chalk)",
  };
}

/* One consumer for the sidebar plane. The titlebar band and every row already
 * read the nav-plane family, so changing those variables reaches the whole
 * left surface without rebuilding its DOM or teaching individual rows about
 * appearance modes. */
function paintLeftSidebarAppearance() {
  if (!leftSidebarAppearanceSpec) return;
  const root = document.documentElement;
  const surface = leftSidebarSurface();
  root.dataset.leftSidebarAppearance = leftSidebarAppearanceMode;
  if (surface === null) {
    for (const property of LEFT_SIDEBAR_SURFACE_PROPERTIES) {
      root.style.removeProperty(property);
    }
  } else {
    root.style.setProperty("--nav-plane", surface.background);
    root.style.setProperty("--nav-plane-ink", surface.foreground);
    root.style.setProperty(
      "--nav-plane-accent",
      "color-mix(in srgb, var(--nav-plane-ink) 9%, var(--nav-plane))",
    );
    root.style.setProperty(
      "--nav-plane-rule",
      "color-mix(in srgb, var(--nav-plane-ink) 7%, var(--nav-plane))",
    );
    root.style.setProperty(
      "--nav-plane-ring",
      "color-mix(in srgb, var(--nav-plane-ink) 44%, var(--nav-plane))",
    );
    root.style.setProperty(
      "--left-sidebar-muted-ink",
      "color-mix(in srgb, var(--nav-plane-ink) 62%, var(--nav-plane))",
    );
  }

  for (const button of document.querySelectorAll("button[data-left-sidebar-mode]")) {
    const active = button.dataset.leftSidebarMode === leftSidebarAppearanceMode;
    button.classList.toggle("is-active", active);
    button.setAttribute("aria-pressed", String(active));
  }
  el("left-sidebar-tint-settings").hidden = leftSidebarAppearanceMode !== "tinted";
  el("left-sidebar-tint-color").value = leftSidebarTintColor;
  el("left-sidebar-tint-swatch").value = leftSidebarTintColor;

  const opacity = el("left-sidebar-tint-opacity");
  const opacitySpec = leftSidebarAppearanceSpec.tint_opacity;
  opacity.min = String(opacitySpec.min);
  opacity.max = String(opacitySpec.max);
  opacity.step = String(opacitySpec.step);
  opacity.value = String(leftSidebarTintOpacity);
  el("left-sidebar-tint-range").textContent = `${opacitySpec.min} to ${opacitySpec.max}`;
}

function patchLeftSidebarAppearance(kind, value) {
  const keyByKind = {
    mode: "left_sidebar_appearance_mode",
    tint_color: "left_sidebar_tint_color",
    tint_opacity: "left_sidebar_tint_opacity",
  };
  if (kind === "mode") leftSidebarAppearanceMode = normalizedLeftSidebarAppearanceMode(value);
  else if (kind === "tint_color") leftSidebarTintColor = normalizedLeftSidebarTintColor(value);
  else if (kind === "tint_opacity") {
    leftSidebarTintOpacity = normalizedLeftSidebarTintOpacity(value);
  } else {
    throw new Error(`unknown left-sidebar appearance patch: ${kind}`);
  }
  paintLeftSidebarAppearance();
  const canonical = {
    mode: leftSidebarAppearanceMode,
    tint_color: leftSidebarTintColor,
    tint_opacity: leftSidebarTintOpacity,
  }[kind];
  return commitSetting(keyByKind[kind], "patch_left_sidebar_appearance", {
    patch: { kind, value: canonical },
  });
}

for (const button of document.querySelectorAll("button[data-left-sidebar-mode]")) {
  button.addEventListener("click", () => {
    void patchLeftSidebarAppearance("mode", button.dataset.leftSidebarMode);
  });
}

el("left-sidebar-tint-swatch").addEventListener("input", (event) => {
  void patchLeftSidebarAppearance("tint_color", event.target.value);
});

el("left-sidebar-tint-color").addEventListener("change", (event) => {
  void patchLeftSidebarAppearance("tint_color", event.target.value);
});

el("left-sidebar-tint-opacity").addEventListener("change", (event) => {
  void patchLeftSidebarAppearance("tint_opacity", event.target.value);
});

const CSS_GENERIC_FONT_FAMILIES = new Set([
  "serif",
  "sans-serif",
  "monospace",
  "cursive",
  "fantasy",
  "system-ui",
  "blinkmacsystemfont",
]);

/* What a person typed into a font field, as a `font-family` value. They type
 * the way CSS is written: one face, a face in quotes, or a list — and each of
 * those must stay what it was. Escaping the whole text as one name turned
 * `"Pretendard"` into a face whose name has quote marks in it and `A, B` into
 * one face called "A, B" — neither exists, and the page fell to the engine's
 * default face (t-6323 A0). So: split on the commas outside quotes, take one
 * layer of quotes off each name, leave the generic families and the
 * platform's `-apple-system`-style keywords bare, and escape the rest. */
function cssFontFamilyChoice(family) {
  const names = [];
  let name = "";
  let quote = null;
  for (const glyph of String(family ?? "")) {
    if (quote) {
      if (glyph === quote) quote = null;
      else name += glyph;
    } else if (glyph === "\"" || glyph === "'") {
      quote = glyph;
    } else if (glyph === ",") {
      names.push(name);
      name = "";
    } else {
      name += glyph;
    }
  }
  names.push(name);
  return names
    .map((one) => one.trim())
    .filter(Boolean)
    .map((one) => (one.startsWith("-") || CSS_GENERIC_FONT_FAMILIES.has(one.toLocaleLowerCase())
      ? one
      : CSS.escape(one)))
    .join(", ");
}

function applyAppFontFamily() {
  const root = document.documentElement;
  if (!appFontFamily) {
    root.style.removeProperty("--font-ui-choice");
    return;
  }
  root.style.setProperty("--font-ui-choice", cssFontFamilyChoice(appFontFamily));
}

function paintAppFontFamily() {
  el("app-font-family").value = appFontFamily;
  applyAppFontFamily();
}

function setAppFontFamily(family) {
  appFontFamily = family.trim();
  paintAppFontFamily();
  void commitSetting("app_font_family", "set_app_font_family", { family: appFontFamily });
}

el("app-font-family").addEventListener("change", (event) => {
  const family = event.target.value.trim();
  if (family === appFontFamily) {
    paintAppFontFamily();
    return;
  }
  setAppFontFamily(family);
});

function paintTitlebarAppName() {
  el("titlebar-app-name").hidden = !showTitlebarAppName;
  el("show-titlebar-app-name").checked = showTitlebarAppName;
}

function setShowTitlebarAppName(visible) {
  showTitlebarAppName = Boolean(visible);
  paintTitlebarAppName();
  void commitSetting("show_titlebar_app_name", "set_show_titlebar_app_name", {
    visible: showTitlebarAppName,
  });
}

el("show-titlebar-app-name").addEventListener("change", (event) => {
  setShowTitlebarAppName(event.target.checked);
});

function paintMenuBarIconPreference() {
  el("show-menu-bar-icon-row").hidden = !usesCommandModifier;
  el("show-menu-bar-icon").checked = showMenuBarIcon;
}

function setShowMenuBarIcon(visible) {
  showMenuBarIcon = Boolean(visible);
  paintMenuBarIconPreference();
  void commitSetting("show_menu_bar_icon", "set_show_menu_bar_icon", {
    visible: showMenuBarIcon,
  });
}

el("show-menu-bar-icon").addEventListener("change", (event) => {
  setShowMenuBarIcon(event.target.checked);
});

function paintMinimizeToTrayPreference() {
  el("minimize-to-tray-row").hidden = !usesWindowsPlatform;
  el("minimize-to-tray-on-close").checked = minimizeToTrayOnClose;
}

function setMinimizeToTrayOnClose(enabled) {
  minimizeToTrayOnClose = Boolean(enabled);
  paintMinimizeToTrayPreference();
  void commitSetting("minimize_to_tray_on_close", "set_minimize_to_tray_on_close", {
    enabled: minimizeToTrayOnClose,
  });
}

el("minimize-to-tray-on-close").addEventListener("change", (event) => {
  setMinimizeToTrayOnClose(event.target.checked);
});

el("titlebar-app-name").addEventListener("contextmenu", (event) => {
  event.preventDefault();
  openSidebarMenu(event.clientX, event.clientY, [{
    label: t("titlebar.hideAppName", "앱 이름 숨기기"),
    run: () => setShowTitlebarAppName(false),
  }]);
});

/* 워크스페이스 카드가 그릴 줄 아는 것들. `{ id, key, name }` 데이터 행인
 * 이유는 게이트가 아니라 언어다 — 낱말을 인자로 흩으면 한 언어로 굳는다. */
const WORKTREE_CARD_PROPERTIES = [
  { id: "branch", key: "sidebar.cardBranch", name: "브랜치 이름" },
  { id: "ports", key: "sidebar.cardPorts", name: "포트" },
  { id: "agents", key: "sidebar.cardAgents", name: "에이전트 상태" },
  { id: "default-badge", key: "sidebar.cardDefaultBadge", name: "기본 배지" },
];

const worktreeCardProperties = new Set(WORKTREE_CARD_PROPERTIES.map((one) => one.id));

/* 카드가 **그리지 않는** 것을, 토큰이 묶여 있는 루트에 적는다.
 *
 * 켠 것이 아니라 끈 것을 적는 것은 방향이 중요해서다: 이 창이 내일 그릴 줄 알게
 * 될 속성은 이 메뉴를 한 번도 연 적 없는 사람에게 **보이는 채로** 와야 한다.
 *
 * 그리고 이 길은 목록을 다시 짓지 않는다 — CSS 한 줄이 답할 수 있는 물음에
 * 사이드바 전체를 다시 세우는 것이 이 슬라이스가 피한 값이다. */
function paintWorktreeCardProperties() {
  document.documentElement.dataset.worktreeCardOff = WORKTREE_CARD_PROPERTIES
    .filter((one) => !worktreeCardProperties.has(one.id))
    .map((one) => one.id)
    .join(" ");
}

function setWorktreeCardProperty(property, shown) {
  if (shown) worktreeCardProperties.add(property);
  else worktreeCardProperties.delete(property);
  paintWorktreeCardProperties();
  void commitSetting(
    "worktree_card_properties",
    "set_worktree_card_property",
    { property, shown },
  );
}

function paintWorktreeCardLayout() {
  const mode = compactWorktreeCards ? "compact" : "detailed";
  document.documentElement.dataset.worktreeCardLayout = mode;
  for (const button of document.querySelectorAll("button[data-worktree-card-layout]")) {
    const active = button.dataset.worktreeCardLayout === mode;
    button.classList.toggle("is-active", active);
    button.setAttribute("aria-pressed", String(active));
  }
}

function setCompactWorktreeCards(compact) {
  compactWorktreeCards = Boolean(compact);
  paintWorktreeCardLayout();
  void commitSetting(
    "compact_worktree_cards",
    "set_compact_worktree_cards",
    { compact: compactWorktreeCards },
  );
}

for (const button of document.querySelectorAll("button[data-worktree-card-layout]")) {
  button.addEventListener("click", () => {
    setCompactWorktreeCards(button.dataset.worktreeCardLayout === "compact");
  });
}

/* 에이전트 활동 레이아웃 — 표시 메뉴의 그 세그먼트가 부르는 하나의 문.
 * 다른 카드 설정처럼 그림이 먼저고 쓰기는 그 뒤다: 목록은 사람이 보고 있는
 * 것이고, 메뉴가 한 일을 파일이 답할 때까지 기다리게 하면 고장난 메뉴다. */
function setAgentActivityDisplay(mode) {
  agentActivityDisplay = mode === "full" ? "full" : "compact";
  paintWorktreeAgents();
  void commitSetting(
    "agent_activity_display",
    "set_agent_activity_display",
    { mode: agentActivityDisplay },
  );
}

/* Whether the machine is asking for light right now. */
const asksForLight = window.matchMedia("(prefers-color-scheme: light)");

/* Put the resolved treatment on the root, where the tokens are bound.
 *
 * Dark removes the attribute rather than setting `data-theme="dark"`: dark is
 * what `:root` already binds, and a second selector saying the same thing is
 * a second place to keep in step. */
function applyTheme() {
  const resolved = theme === "system" ? (asksForLight.matches ? "light" : "dark") : theme;
  if (resolved === "light") document.documentElement.dataset.theme = "light";
  else delete document.documentElement.dataset.theme;
  // The terminal palette flips with the treatment too, and a colour the
  // contrast lift rewrote is a literal that stopped following its token — so
  // the screens repaint from their models rather than holding the old palette
  // until the program inside happens to rewrite those rows.
  if (!applyTerminalPalette()) forgetPalette();
}

/* The desktop switching from light to dark under a window set to `시스템
 * 설정` is the whole reason that option exists, so it is listened for rather
 * than read once at boot. */
asksForLight.addEventListener("change", () => {
  if (theme === "system") applyTheme();
});

function paintThemePicker() {
  themePicker.replaceChildren();
  for (const choice of THEMES) {
    const option = document.createElement("option");
    option.value = choice.code;
    option.textContent = t(choice.key, choice.name);
    option.selected = choice.code === theme;
    themePicker.appendChild(option);
  }
}

function setTheme(code) {
  theme = code;
  applyTheme();
  paintThemePicker();
  void commitSetting("theme", "set_theme", { code });
}

themePicker.addEventListener("change", () => setTheme(themePicker.value));

/* ---- the window's own material (Orca's Settings ▸ Window) ----
 *
 * Two settings, both measured (`TerminalWindowSection`,
 * Settings-Db2c7R7D.js:5150): how much of what is behind the terminal shows
 * through it, and whether the window itself is blurred glass. The blur needs
 * the process to have asked the operating system for a material at startup,
 * so like Orca's it says so and offers the relaunch rather than pretending to
 * take effect. */
const OPACITY_DEFAULT = 1;
let terminalOpacity = OPACITY_DEFAULT;
let windowBlur = false;
/* What the PROCESS started with, which is the only thing the material can
 * actually be right now — the switch below is a stored intent until then. */
let blurAtBoot = false;

/* The screen alpha, as a number the stylesheet can multiply into the terminal
 * background. Written onto the root rather than baked into `--term-screen-bg`
 * so a theme change and an opacity change stay independent. */
function applyWindowMaterial() {
  document.documentElement.style.setProperty("--term-screen-alpha", String(terminalOpacity));
  // The chrome turns to glass only where there is something behind it, which
  // is exactly when the window was made translucent at startup.
  if (blurAtBoot) document.documentElement.dataset.blur = "on";
  else delete document.documentElement.dataset.blur;
  el("blur-restart").hidden = windowBlur === blurAtBoot;
}

function paintWindowControls() {
  el("window-opacity").value = String(terminalOpacity);
  el("window-blur").checked = windowBlur;
}

el("window-opacity").addEventListener("change", () => {
  const asked = Number(el("window-opacity").value);
  // A field a person types into can hold anything; the stored value cannot.
  terminalOpacity = Number.isFinite(asked) ? Math.min(1, Math.max(0, asked)) : OPACITY_DEFAULT;
  paintWindowControls();
  applyWindowMaterial();
  void commitSetting("window_material", "set_terminal_opacity", { value: terminalOpacity });
});

el("window-blur").addEventListener("change", () => {
  windowBlur = el("window-blur").checked;
  applyWindowMaterial();
  void commitSetting("window_material", "set_window_blur", { on: windowBlur });
});

el("blur-relaunch").addEventListener("click", () => {
  void askBeforeRestart("window-material");
});

/* ---- 「새 빌드 준비됨」, the settings half ----
 *
 * The same primitive as blur-restart above and the same button: the release
 * lane installed a build this process is not running, so the General page
 * says the toast's own sentence (`updateReadyAppWords`, shell-status.js —
 * 「새 버전 {{version}}」 with the sha dim beside it, or 「새 빌드({{sha}})」,
 * t-3237), names the build this process runs under it with t-3191's one
 * running line, and offers the one restart road. Painted from the toast's
 * source (`releaseNotice`) and on every language change, since the words
 * are interpolated at paint time rather than read off `data-i18n`. Only zo
 * changed: the zo line and no button — restarting the window would change
 * nothing.
 *
 * The feed's 「준비됨」 (t-3191) stands on this same surface: a version staged
 * beside the bundle (`updateReadyVersion`, shell-update.js) takes the app
 * line and the same button, since the swap happens on that one restart road. */
function paintUpdateNotice() {
  const app = releaseNotice?.app ?? null;
  const zo = releaseNotice?.zo ?? null;
  const ready = updateReadyVersion();
  el("update-notice").hidden = !(app || zo || ready);
  const appLine = el("update-notice-app");
  appLine.hidden = !(app || ready);
  if (app) {
    const { sentence, aux } = updateReadyAppWords(app);
    speakWithAux(appLine, sentence, aux, "settings-notice-aux");
  } else {
    appLine.textContent = ready
      ? t("settings.update.readyVersion", "버전 {{version}} 준비됨 · 다시 시작하면 적용됩니다", {
          version: ready,
        })
      : "";
  }
  // The build this process runs, under the sentence: the update pane's own
  // words (`updateRunningWords`, shell-update.js) — one key, never a second.
  const runningLine = el("update-notice-running");
  runningLine.hidden = !app;
  runningLine.textContent = app
    ? updateRunningWords({ version: app.running_version, commit: app.running })
    : "";
  const zoLine = el("update-notice-zo");
  zoLine.hidden = !zo;
  if (zo) {
    const { sentence, aux } = updateReadyZoWords(zo);
    speakWithAux(zoLine, sentence, aux, "settings-notice-aux");
  } else {
    zoLine.textContent = "";
  }
  // Who a restart would cut (t-3058): said only beside the restart button,
  // whichever road shows it — the lane's build or a staged update.
  const workersLine = el("update-notice-workers");
  const busy = app || ready ? updateWorkersWords() : "";
  workersLine.hidden = busy === "";
  workersLine.textContent = busy;
  el("update-relaunch").hidden = !(app || ready);
}

el("update-relaunch").addEventListener("click", () => {
  void askBeforeRestart("settings-notice");
});

/* ---- the agents this machine can run ----
 *
 * Two halves crossed. The registry in `zerocode-core` says how to drive
 * thirty-five agents — what to launch, what process to expect, and what each
 * one does when it is ready for keys; `PATH` says which are actually here.
 * Neither half is a list of choices on its own: thirty-five names is not a
 * menu, and offering an agent that is not installed makes the first thing
 * somebody tries fail.
 *
 * `zo` is one of the thirty-five. It is the one agent this window also drives
 * over a socket as a lane, and for a while that was the only way it appeared
 * anywhere — the lane catalogue knew it and this one did not, so the default
 * that decides what a new terminal starts had no row for the agent this
 * project ships.
 *
 * `AGENT_AUTO` and `AGENT_BLANK` are Orca's own two extra pills. `자동` means
 * "whatever the terminal command setting says", which is this window's existing
 * behaviour and therefore the default default. `에이전트 없음` is a plain shell,
 * chosen deliberately rather than by having no agents installed. */
const AGENT_AUTO = Object.freeze({ kind: "auto" });
const AGENT_BLANK = Object.freeze({ kind: "blank" });

function agentPreference(id) {
  return { kind: "agent", id };
}

/* One migration boundary for boot reports and old browser fixtures. Internal
 * state is always the tagged Rust wire type; no consumer interprets null or a
 * magic agent id on its own. */
function normalizeDefaultAgent(preference) {
  if (preference === null || preference === undefined || preference?.kind === "auto") {
    return AGENT_AUTO;
  }
  if (preference === "blank" || preference?.kind === "blank") return AGENT_BLANK;
  if (typeof preference === "string") return agentPreference(preference);
  if (preference?.kind === "agent" && typeof preference.id === "string") {
    return agentPreference(preference.id);
  }
  return AGENT_AUTO;
}

function defaultAgentId(preference = defaultAgent) {
  return preference?.kind === "agent" ? preference.id : null;
}

function sameDefaultAgent(left, right) {
  return left?.kind === right?.kind && defaultAgentId(left) === defaultAgentId(right);
}

let agentRows = [];
let defaultAgent = AGENT_AUTO;

/* Does the setting NAME an agent — as against 자동, which means the terminal
 * command, and 에이전트 없음, which means a plain shell on purpose? Both
 * questions that turn on it ask it in one spelling: what a new terminal
 * starts, and whether a workspace's stored plain shells give way to it. */
function defaultAgentChosen() {
  return defaultAgentId() !== null;
}

async function refreshAgents(rereadPath = false) {
  try {
    // `refresh: true` makes the backend re-read the shell PATH first — the
    // person pressing the button has usually just installed something, which
    // changed the PATH the last read captured.
    agentRows = (await invoke("list_agents", { refresh: rereadPath })) ?? [];
  } catch (error) {
    showError(error);
    agentRows = [];
  }
  // Read together, painted once. The rows say what each agent will be allowed
  // to do, so a paint between the two reads would flash a list with no badges.
  await refreshLaunchPlans();
  paintAgents();
  // The tabs' identity marks read this catalog too (`renderTab`): a pane that
  // was painted before it answered wears a letter until the next render, and
  // nothing else promises one.
  renderTabs();
}

function installedAgents() {
  return agentRows.filter((row) => row.installed);
}

/* The one place an agent id becomes a person's word for it. The id itself is
 * the honest fallback — a catalog that has not loaded yet still names the
 * pane something true. */
function agentSaidName(id) {
  return agentRows.find((row) => row.id === id)?.name ?? id;
}

/* An agent's icon, resolved the way Orca resolves it (`AgentIcon`,
 * `agent-catalog-1Y3pTpm8.js:452-490`) minus the two steps this repository
 * cannot take. Orca draws nine agents by hand and bundles favicon PNGs for
 * most of the rest — both third-party artwork, which stays out of shipped
 * code — and past those, its chain is a runtime fetch of the vendor's own
 * favicon through the public service, then a letter tile. Every agent here
 * takes that runtime path. The letter is drawn first and the favicon covers
 * it when it arrives, so a machine that is offline shows a full row of tiles
 * rather than a row of broken images. */
/* One mark per domain, however many rows draw it.
 *
 * The pills, the settings rows, the send menu and a launched tab's chip all ask
 * for the same handful of agents, and the backend's own disk cache would still
 * be one round trip each. A promise is stored rather than the answer, so rows
 * that ask while the first fetch is in flight join it instead of starting a
 * second — and a domain that has no mark is remembered as `null`, so a machine
 * offline asks once per agent per window rather than once per repaint. */
const agentMarks = new Map();

function agentMark(domain) {
  if (!agentMarks.has(domain)) {
    agentMarks.set(
      domain,
      invoke("agent_icon", { domain }).catch(() => null),
    );
  }
  return agentMarks.get(domain);
}

/* The face an agent wears when there is no mark: its initial on a tile.
 *
 * Orca's own fallback, and the honest answer rather than a failure — a machine
 * that has never been online for this agent has no mark to show, and a letter
 * naming it beats a broken-image glyph (`AgentLetterIcon`,
 * agent-catalog-1Y3pTpm8.js:128-150). */
function agentLetterTile(row) {
  const letter = document.createElement("span");
  letter.className = "agent-ico-letter";
  letter.textContent = (row.name || row.id).charAt(0).toUpperCase();
  return letter;
}

/* The vendor's real mark, once the backend has it.
 *
 * Separate from building the element because it happens LATER: the tile is on
 * screen immediately and the mark replaces it when it lands, so a list never
 * waits on a network read to paint. */
function paintAgentMark(wrap, domain) {
  void agentMark(domain).then((uri) => {
    // The list may have been rebuilt while this was in flight — a wrapper no
    // longer in the document is nobody's mark.
    if (!uri || !wrap.isConnected) return;
    const image = document.createElement("img");
    image.alt = "";
    image.setAttribute("aria-hidden", "true");
    image.src = uri;
    wrap.prepend(image);
    wrap.classList.remove("is-lettered");
  });
}

/* An agent's face: the tile now, its real mark when there is one.
 *
 * The mark is fetched by the BACKEND and arrives as a `data:` URL. Asking for
 * it from here is what put a letter on every agent for a while: the favicon
 * service answers 301 to another host, and a CSP source carrying a path does
 * not survive a redirect — so every image failed in the app while the browser
 * tests, which have no CSP, showed them all. The window admits no remote image
 * host at all now. See crates/zerocode-shell/src/icon.rs. */
function agentIcon(row) {
  const wrap = document.createElement("span");
  wrap.className = "agent-ico is-lettered";
  wrap.appendChild(agentLetterTile(row));
  // A row without a domain is a placeholder from before the registry loaded;
  // it keeps the letter rather than asking about nothing.
  if (row.favicon_domain) paintAgentMark(wrap, row.favicon_domain);
  return wrap;
}

/* The pill row: 자동, 에이전트 없음, then one per installed agent.
 *
 * Only the installed ones get a pill. An agent that is not here cannot be a
 * default — a stored preference for one is what Orca's `isAutoDefault` falls
 * back out of, and it reads as 자동 until the tool appears. */
function paintAgentPills() {
  const host = el("agent-pills");
  host.replaceChildren();
  const installed = installedAgents();
  const chosenId = defaultAgentId();
  const chosen = chosenId && installed.some((row) => row.id === chosenId)
    ? defaultAgent
    : defaultAgent.kind === "blank" ? AGENT_BLANK : AGENT_AUTO;
  const pill = (preference, label, front) => {
    const button = document.createElement("button");
    button.className = "agent-pill";
    button.type = "button";
    button.dataset.agent = defaultAgentId(preference) ?? preference.kind;
    button.setAttribute("aria-pressed", sameDefaultAgent(preference, chosen) ? "true" : "false");
    const mark = document.createElement("span");
    mark.className = "agent-pill-mark";
    mark.innerHTML = icon("check");
    const name = document.createElement("span");
    name.className = "agent-pill-label";
    name.textContent = label;
    // Orca leads 자동's pill with its check and ends every other pill with
    // one (`AgentsPane`, plugin-command-keybindings-BRnuynSD.js:1494-1521) —
    // the auto pill is the only one with no icon to open with.
    if (front) button.append(front, name, mark);
    else button.append(mark, name);
    button.addEventListener("click", () => chooseDefaultAgent(preference));
    host.appendChild(button);
  };
  pill(AGENT_AUTO, t("settings.agents.auto", "자동"));
  const blank = document.createElement("span");
  blank.className = "agent-ico";
  blank.innerHTML = icon("terminal");
  pill(AGENT_BLANK, t("settings.agents.blank", "에이전트 없음"), blank);
  for (const row of installed) pill(agentPreference(row.id), row.name, agentIcon(row));
}

/* What each agent will be launched with, keyed by id. Read once when the
 * settings pane opens; the rows are drawn from it and a save refreshes it. */
let launchPlans = new Map();

async function refreshLaunchPlans() {
  try {
    const rows = await invoke("agent_launch_plans");
    launchPlans = new Map((rows ?? []).map((one) => [one.agent, one]));
  } catch (error) {
    // A window that cannot read this still lists its agents — it just cannot
    // say what they will be allowed to do, and saying nothing is honest.
    launchPlans = new Map();
  }
}

/* Orca's global permission summary. Its segmented control deliberately shows
 * Manual only when every permission-bearing agent is manual; a mixture or a
 * custom override shows Yolo, while the explanatory copy says custom launch
 * overrides are left alone. The backend remains the only owner of which
 * args/env values actually count as either mode. */
function agentPermissionModeSummary() {
  const modes = [...launchPlans.values()]
    .filter((plan) => plan.has_switch)
    .map((plan) => plan.permission);
  if (modes.length === 0) return null;
  return modes.every((mode) => mode === "asks") ? "manual" : "yolo";
}

let agentPermissionWriteInFlight = false;

function paintAgentPermissionMode() {
  const group = el("agent-permission-choice");
  const current = agentPermissionModeSummary();
  group.setAttribute("aria-busy", String(agentPermissionWriteInFlight));
  for (const button of group.querySelectorAll("[data-mode]")) {
    const active = current !== null && button.dataset.mode === current;
    button.classList.toggle("is-active", active);
    button.setAttribute("aria-pressed", String(active));
    button.disabled = current === null || agentPermissionWriteInFlight;
  }
}

/* One gesture, one repository transaction. A refusal and a committed write
 * whose acknowledgement was lost both finish by re-reading the canonical
 * plans, exactly like a per-agent launch-line edit. */
async function setAgentPermissionMode(mode) {
  if (agentPermissionWriteInFlight || agentPermissionModeSummary() === mode) return;
  agentPermissionWriteInFlight = true;
  paintAgentPermissionMode();
  let failure = null;
  try {
    await invoke("set_agent_permission_mode", { mode });
  } catch (error) {
    failure = error;
  }
  await refreshLaunchPlans();
  agentPermissionWriteInFlight = false;
  paintAgents();
  if (failure !== null) showError(String(failure));
}

/* What a launch will be allowed to do, in one word.
 *
 * This is the half of "connected" a person cannot otherwise see. An agent
 * started without its bypass flag stops to ask before it edits anything, and
 * the question appears in a pane that may be behind another tab — so the work
 * silently never happens. Orca hands the flag by default and keeps the fact in
 * a table; this window hands it by default and SAYS so. */
function permissionBadge(row) {
  const plan = launchPlans.get(row.id);
  if (!plan?.has_switch) return null;
  const said = {
    unattended: t("settings.agents.unattended", "묻지 않고 진행"),
    asks: t("settings.agents.asks", "매번 물어봄"),
    mixed: t("settings.agents.mixedPerms", "직접 지정함"),
  };
  const badge = document.createElement("span");
  badge.className = "agent-perm";
  badge.dataset.mode = plan.permission;
  badge.textContent = said[plan.permission] ?? plan.permission;
  return badge;
}

/* One row per agent, in either list. The command is shown for the ones that
 * are NOT here, because that is the actionable half — put this on PATH and it
 * moves up. For the ones that are, the name it was found under is shown only
 * when it differs from the agent's own, which is what an alias is. */
/* One cached record and renderer for Settings and the board inspector. Details
 * are fetched only when opened; ordinary cards and status beats add no RPC. */
const zoIntegrationRecords = new Map();
const zoIntegrationViews = new Set();

function zoIntegrationReason(reason) {
  const words = {
    discovering: () => t("zo.integration.discovering", "zo에 연결 중…"),
    verified: () => t("zo.integration.verified", "연결됨"),
    "method-not-found": () => t("zo.integration.legacy", "연결됨 · 이전 zo; 상세 정보 없음"),
    "discovery-missing": () => t("zo.integration.discoveryMissing", "채널 기록 없음"),
    "discovery-invalid": () => t("zo.integration.discoveryInvalid", "채널 기록이 잘못됨"),
    "discovery-stale": () => t("zo.integration.discoveryStale", "채널 기록이 오래됨"),
    "connect-refused": () => t("zo.integration.connectRefused", "채널 연결 거부됨"),
    "auth-rejected": () => t("zo.integration.authRejected", "채널 인증 거부됨"),
    "info-invalid": () => t("zo.integration.infoInvalid", "세션 정보가 잘못됨"),
    "capabilities-invalid": () => t("zo.integration.capabilitiesInvalid", "기능 정보가 잘못됨"),
    "capabilities-timeout": () => t("zo.integration.capabilitiesTimeout", "기능 확인 시간 초과"),
    "protocol-unsupported": () => t("zo.integration.protocolUnsupported", "지원하지 않는 채널 버전"),
    "owner-conflict": () => t("zo.integration.ownerConflict", "다른 판이 소유한 채널"),
    "session-mismatch": () => t("zo.integration.sessionMismatch", "세션이 일치하지 않음"),
    "process-replaced": () => t("zo.integration.processReplaced", "프로세스가 바뀜"),
    disconnected: () => t("zo.integration.disconnected", "채널 연결 끊김"),
    "launch-refused": () => t("zo.integration.launchRefused", "실행 거부됨"),
  };
  return words[reason]?.() ?? t("zo.integration.unknown", "아직 확인되지 않음");
}

function zoIntegrationSummary(record) {
  if (!record) return t("zo.integration.unknown", "아직 확인되지 않음");
  const words = [zoIntegrationReason(record.reason)];
  // The exact launch contract's verdict (t-2773): a refusal names its reason
  // code; a briefing the window withheld is a second sentence. An accepted
  // contract and a legacy launch add nothing to "Connected".
  const contract = record.receipt?.launch?.contract;
  if (contract?.verdict === "refused") {
    words.push(t("zo.integration.contractRefused", "정확 실행 계약 거부 · {{reason}}",
      { reason: contract.reason ?? t("zo.integration.unknown", "아직 확인되지 않음") }));
  }
  if (record.delivery?.refused) words.push(t("zo.integration.briefingWithheld", "브리핑을 전달하지 않았습니다"));
  const mode = record.receipt?.mode;
  if (mode?.reason === "no-spawn") {
    words.push(t("zo.integration.noSpawn", "헬퍼 실행이 꺼짐"));
  } else if (mode?.requested === "panes" && mode?.available === false) {
    words.push(t("zo.integration.panesUnavailable", "판 모드 요청됨 · 사용할 수 없음"));
  } else if (mode?.effective === "inline") {
    words.push(t("zo.integration.inline", "헬퍼는 인라인으로 실행"));
  }
  if (record.source_differs === true) words.push(t("zo.integration.sourceDiffers", "선택한 소스와 실행 빌드가 다름"));
  if (record.installed_changed === true) words.push(t("zo.integration.installedChanged", "설치 파일이 바뀜 · 이 판은 이전 빌드 실행 중"));
  if (record.stale) words.push(t("zo.integration.stale", "마지막 관측값"));
  return words.join(" · ");
}

function rememberZoIntegration(record) {
  if (!record || typeof record.pane !== "string") return;
  const old = zoIntegrationRecords.get(record.pane);
  if (old && old.updated_at > record.updated_at) return;
  zoIntegrationRecords.set(record.pane, record);
  for (const view of zoIntegrationViews) {
    if (!view.isConnected) { zoIntegrationViews.delete(view); continue; }
    if (view.dataset.pane === record.pane || view.dataset.pane === "") {
      if (view.dataset.pane) view.querySelector("summary").textContent = zoIntegrationSummary(record);
      view.refreshIntegration?.();
    }
  }
}

listen("zo:integration", (event) => rememberZoIntegration(event.payload));

function zoIntegrationNode(pane = null) {
  const root = document.createElement("details");
  root.className = "zo-integration";
  root.dataset.pane = pane ?? "";
  const summary = document.createElement("summary");
  summary.textContent = pane ? zoIntegrationSummary(zoIntegrationRecords.get(pane))
    : t("zo.integration.details", "연동 상세 정보");
  const body = document.createElement("div");
  body.className = "zo-integration-body";
  root.append(summary, body);
  zoIntegrationViews.add(root);
  let generation = 0;
  async function load(sourceRevision = null) {
    const asking = ++generation;
    try {
      const report = await invoke("zo_integration_details", { pane, sourceRevision, export: false });
      if (asking !== generation || !root.isConnected) return;
      for (const record of report?.records ?? []) rememberZoIntegration(record);
      body.replaceChildren();
      const liveRows = [];
      const row = (label, value) => {
        const line = document.createElement("p");
        const name = document.createElement("strong");
        name.textContent = `${label}: `;
        const text = document.createElement("span");
        text.textContent = value == null ? t("zo.integration.unknown", "아직 확인되지 않음")
          : typeof value === "object" ? JSON.stringify(value) : String(value);
        line.append(name, text); body.appendChild(line);
        return text;
      };
      row(t("zo.integration.window", "창 빌드"), report?.window);
      row(t("zo.integration.installed", "설치된 실행 파일"), report?.installed_path);
      if (!report?.installed) row(t("zo.integration.state", "연결 상태"), t("zo.integration.notInstalled", "zo가 설치되지 않음"));
      for (const record of report?.records ?? []) {
        row(t("zo.integration.pane", "판"), record.pane);
        const stateText = row(t("zo.integration.state", "연결 상태"), zoIntegrationSummary(record));
        liveRows.push(() => { stateText.textContent = zoIntegrationSummary(zoIntegrationRecords.get(record.pane) ?? record); });
        row(t("zo.integration.updated", "마지막 관측"), record.updated_at ? new Date(record.updated_at).toLocaleString() : null);
        row(t("zo.integration.launchPath", "실행 시 경로"), record.launch_path);
        row(t("zo.integration.process", "실행 프로세스 빌드"), record.receipt?.process);
        row(t("zo.integration.protocol", "채널 규약"), record.receipt?.protocol);
        row(t("zo.integration.requested", "요청한 선택"), record.receipt?.launch?.requested);
        const effectiveText = row(t("zo.integration.effective", "관측된 선택"), record.receipt?.current?.effective ?? record.receipt?.launch?.effective);
        liveRows.push(() => { const latest = zoIntegrationRecords.get(record.pane) ?? record; effectiveText.textContent = JSON.stringify(latest.receipt?.current?.effective ?? latest.receipt?.launch?.effective ?? null); });
        row(t("zo.integration.contract", "실행 계약"), record.receipt?.launch?.contract);
        row(t("zo.integration.delivery", "브리핑 전달"), record.delivery);
        row(t("zo.integration.hostMode", "창이 전달한 모드 근거"), record.configured_mode);
        row(t("zo.integration.mode", "설정 및 관측 모드"), record.receipt?.mode);
        row(t("zo.integration.source", "선택한 소스 리비전"), record.source_revision);
      }
      root.refreshIntegration = () => liveRows.forEach((refresh) => refresh());
      const label = document.createElement("label");
      label.textContent = t("zo.integration.source", "선택한 소스 리비전");
      const input = document.createElement("input");
      input.type = "text"; input.maxLength = 40; input.pattern = "[a-fA-F0-9]{40}";
      input.placeholder = t("zo.integration.sourceHint", "선택 사항: 비교할 zo 소스의 전체 Git SHA");
      input.value = sourceRevision ?? report?.records?.[0]?.source_revision ?? "";
      label.appendChild(input); body.appendChild(label);
      const compare = document.createElement("button"); compare.type = "button";
      compare.textContent = t("zo.integration.compare", "소스 비교");
      compare.onclick = () => { if (input.reportValidity()) void load(input.value); };
      const copy = document.createElement("button"); copy.type = "button";
      copy.textContent = t("zo.integration.copy", "진단 복사");
      copy.onclick = async () => {
        try {
          copy.disabled = true;
          const safe = await invoke("zo_integration_details", { pane, sourceRevision: null, export: true });
          await clipboardText.write(JSON.stringify(safe, null, 2));
        } catch { showError(t("zo.integration.readFailed", "연동 정보를 읽지 못했습니다")); }
        finally { copy.disabled = false; }
      };
      body.append(compare, copy);
    } catch {
      body.textContent = t("zo.integration.readFailed", "연동 정보를 읽지 못했습니다");
    }
  }
  root.addEventListener("toggle", () => { if (root.open) void load(); });
  return root;
}

function agentRow(row) {
  const line = document.createElement("div");
  line.className = "agent-row";
  // Orca's row shape (`AgentRow`, plugin-command-keybindings-BRnuynSD.js:
  // 1284-1330): a bordered icon chip, then the name with its command in mono
  // beneath it, then the controls at the far end.
  line.innerHTML =
    '<span class="agent-chip"></span><span class="agent-row-body">' +
    '<span class="agent-row-name"></span><span class="agent-row-cmd"></span>' +
    '</span><span class="agent-row-note"></span>';
  line.querySelector(".agent-chip").appendChild(agentIcon(row));
  // The launch line, editable — but only for an agent that is actually here.
  // Offering it for one that is not installed would be a setting for something
  // that cannot run.
  if (row.installed) {
    const badge = permissionBadge(row);
    if (badge) {
      line.querySelector(".agent-row-body").appendChild(launchLineEl(row, badge));
    }
  }
  line.querySelector(".agent-row-name").textContent = row.name;
  const command = line.querySelector(".agent-row-cmd");
  if (row.installed) command.textContent = row.found_as ?? "";
  else command.textContent = row.found_as ?? row.id;
  const note = line.querySelector(".agent-row-note");
  if (row.unsupported_here) {
    note.textContent = t("settings.agents.unsupported", "이 플랫폼에서는 실행되지 않습니다");
  } else if (row.missing_requirement) {
    note.textContent = t("settings.agents.missing", "{{cmd}}이(가) 필요합니다", {
      cmd: row.missing_requirement,
    });
  } else if (row.installed && row.readiness?.auth === "unauthorized") {
    // The readiness snapshot's word (t-3996): every local witness the
    // agent's row allows said there is no login, so a launch would open on
    // a login screen. Said here, before the launch, with the witnesses'
    // own words behind a hover — and only on `unauthorized`: `unknown` is
    // "nobody could look", which accuses nobody.
    note.textContent = t("settings.agents.loginNeeded", "로그인 필요");
    note.classList.add("is-login-needed");
    note.dataset.tip = row.readiness.evidence ?? "";
  } else if (!row.takes_a_paste) {
    // Worth saying, because it changes what this window does with a prompt:
    // this agent is handed it at startup and is never typed at afterwards.
    note.textContent = t("settings.agents.prefilled", "시작할 때 프롬프트를 받습니다");
  } else {
    note.textContent = "";
  }
  // Orca ends every row with the vendor's page behind an external-link icon
  // (`AgentRow`, plugin-command-keybindings-Do9s02Z7.js:1290). One URL, two
  // names: docs for an agent that is here, install instructions for one that
  // is not — which is what makes the "available" list actionable instead of
  // a label. Through `openExternal`, so the backend's scheme guard is the one
  // that decides what a browser may be handed. No link for a row with no
  // vendor page (`zo`): a button leading nowhere is worse than none.
  if (row.homepage_url) {
    const away = document.createElement("button");
    away.className = "agent-row-link";
    away.type = "button";
    away.innerHTML = icon("external");
    const said = row.installed
      ? t("settings.agents.docs", "문서")
      : t("settings.agents.install", "설치");
    away.dataset.tip = said;
    away.setAttribute("aria-label", said);
    away.addEventListener("click", () => openExternal(row.homepage_url));
    line.appendChild(away);
  }
  if (row.id === "zo") line.querySelector(".agent-row-body").appendChild(zoIntegrationNode());
  // An AVAILABLE row is a door in its whole width — a recorded deviation from
  // Orca, whose `AgentRow` clicks nowhere. The end icon alone was reported
  // missed three times over ("설치 클릭해도 반응없고", "에이전트 클릭해도
  // 이동안하고"): on a row whose only verb is "go get it", the 28px icon is
  // the target a person aims past. Installed rows stay plain — their body
  // holds the launch line, and a field that navigates when missed would be
  // worse than the small door was. The guard keeps the inner link from
  // firing the same URL twice.
  if (!row.installed && row.homepage_url) {
    line.classList.add("agent-row-door");
    line.setAttribute("role", "link");
    line.tabIndex = 0;
    line.setAttribute(
      "aria-label",
      `${row.name} — ${t("settings.agents.install", "설치")}`,
    );
    line.addEventListener("click", (event) => {
      if (event.target.closest(".agent-row-link")) return;
      openExternal(row.homepage_url);
    });
    line.addEventListener("keydown", (event) => {
      if (event.key !== "Enter" && event.key !== " ") return;
      event.preventDefault();
      openExternal(row.homepage_url);
    });
  }
  return line;
}

/* The launch line: the badge, the arguments as one editable field, and the way
 * back to the default.
 *
 * One field rather than a flag-by-flag editor because that is what the value
 * IS — a command line, kept as typed. Orca stores it as a string too
 * (`agentDefaultArgs[agent]`) and sanitises on the way in; so does the
 * backend, which is why an agent that refuses a flag never has it in the
 * file. */
function launchLineEl(row, badge) {
  const plan = launchPlans.get(row.id);
  const host = document.createElement("div");
  host.className = "agent-launch";
  host.appendChild(badge);
  // Two fields, one grammar. The args line and the environment line commit
  // the same way and are stored in the same document — but through two doors,
  // because a single door could not tell "I am not editing that half" from
  // "clear that half" and would erase whichever field was not focused.
  const editable = (className, stored, placeholder, label, save) => {
    const field = document.createElement("input");
    field.className = className;
    field.type = "text";
    field.value = stored;
    field.spellcheck = false;
    field.placeholder = placeholder;
    field.setAttribute("aria-label", label);
    // Committed on leaving the field or on Enter, not per keystroke: every
    // character would be a write to disk and a re-read of every agent's plan.
    //
    // Enter writes DIRECTLY rather than by blurring and letting the blur
    // handler do it. `blur()` fires nothing on an element that was never
    // focused, so routing the commit through it made Enter a no-op for any
    // caller that had not clicked into the field first — including every test.
    // No "was this abandoned" flag: Escape puts the stored line BACK, which
    // makes the blur that follows a no-op through the same unchanged-value
    // check that stops Enter from writing twice. One rule, in one place,
    // instead of a second piece of state that can disagree with it.
    const commit = () => save(field.value);
    field.addEventListener("blur", commit);
    field.addEventListener("keydown", (event) => {
      if (event.isComposing) return;
      if (event.key === "Enter") {
        event.preventDefault();
        commit();
        field.blur();
      } else if (event.key === "Escape") {
        event.preventDefault();
        field.value = stored;
        field.blur();
      }
    });
    return field;
  };

  host.appendChild(editable(
    "agent-launch-args",
    plan.args,
    t("settings.agents.argsPlaceholder", "실행 인자 없음"),
    t("settings.agents.args", "실행 인자"),
    (value) => saveAgentLaunch(row.id, value),
  ));
  // The environment line is how an agent is pointed at another model — the
  // CLIs' own supported surface (`ANTHROPIC_BASE_URL`, and Codex's `-c` in the
  // args field beside it). Parsed by the backend, which owns the grammar.
  host.appendChild(editable(
    "agent-launch-env",
    plan.env_line,
    t("settings.agents.envPlaceholder", "환경 변수 없음"),
    t("settings.agents.env", "환경 변수"),
    (value) => saveAgentLaunchEnv(row.id, value),
  ));
  // Only when there is something to go back FROM. A reset offered on a row
  // that is already the default is a control that does nothing.
  if (!plan.is_default) {
    const back = document.createElement("button");
    back.className = "agent-launch-reset";
    back.type = "button";
    back.textContent = t("settings.agents.resetLaunch", "기본값");
    back.addEventListener("click", () => resetAgentLaunch(row.id));
    host.appendChild(back);
  }
  return host;
}

/* Write one half of one agent's launch, or put both halves back.
 *
 * Three doors rather than one, and the reason is the wire: `null` and "key
 * absent" arrive as the same thing, so a single door carrying both halves
 * could never tell "leave that one alone" from "clear it" — and editing the
 * args field would erase an environment override set from the field beside it.
 * Each door names the half it is about, which is what makes its `null`
 * unambiguous.
 *
 * All three land the same way: a refusal and a lost acknowledgement look
 * identical from this webview, so the plans are re-read either way — if the
 * backend committed, its value wins; if it refused, the field rolls back to
 * the last durable value. */
async function commitAgentLaunch(command, args) {
  let failure = null;
  try {
    await invoke(command, args);
  } catch (error) {
    failure = error;
  }
  await refreshLaunchPlans();
  paintAgents();
  if (failure !== null) showError(String(failure));
}

async function saveAgentLaunch(agent, args) {
  const plan = launchPlans.get(agent);
  if (plan && args.trim() === plan.args.trim()) return;
  await commitAgentLaunch("save_agent_launch", { agent, args });
}

async function saveAgentLaunchEnv(agent, env) {
  const plan = launchPlans.get(agent);
  if (plan && env.trim() === plan.env_line.trim()) return;
  await commitAgentLaunch("save_agent_launch_env", { agent, env });
}

async function resetAgentLaunch(agent) {
  await commitAgentLaunch("reset_agent_launch", { agent });
}

function paintAgents() {
  const installed = installedAgents();
  const available = agentRows.filter((row) => !row.installed);
  el("agent-installed-count").textContent = String(installed.length);
  el("agent-available-count").textContent = String(available.length);
  const fill = (host, rows, empty) => {
    host.replaceChildren();
    if (rows.length === 0) {
      const line = document.createElement("p");
      line.className = "agent-row agent-row-name";
      line.textContent = empty;
      host.appendChild(line);
      return;
    }
    for (const row of rows) host.appendChild(agentRow(row));
  };
  fill(
    el("agent-installed"),
    installed,
    t("settings.agents.noneInstalled", "PATH에서 찾은 에이전트가 없습니다"),
  );
  fill(el("agent-available"), available, "");
  paintAgentPills();
  paintAgentPermissionMode();
}

async function chooseDefaultAgent(preference) {
  const asked = normalizeDefaultAgent(preference);
  defaultAgent = asked;
  paintAgentPills();
  await commitSetting("default_agent", "set_default_agent", { preference: asked });
}

el("agent-refresh").addEventListener("click", () => refreshAgents(true));
for (const button of el("agent-permission-choice").querySelectorAll("[data-mode]")) {
  button.addEventListener("click", () => setAgentPermissionMode(button.dataset.mode));
}

/* ---- the agent hook bridge -------------------------------------------------
 *
 * What makes a pane say "this agent is waiting for you" without anybody looking
 * at it. An agent CLI runs a script on its own lifecycle events; this window
 * installs one per agent and listens on loopback. See crates/zerocode-hookd.
 *
 * The section exists because installing means WRITING INTO A FILE THE USER
 * OWNS — `~/.claude/settings.json` and nine more. So each row names its file
 * and says what state it is in, and the switch that turns it on is the same
 * switch that takes the entries back out. A product that edits somebody's
 * settings had better be able to show them what it did. */
let hooksReport = { enabled: true, listening: false, agents: [] };

async function refreshHooks() {
  try {
    hooksReport = (await invoke("hooks_report")) ?? { enabled: true, listening: false, agents: [] };
  } catch (error) {
    showError(error);
    hooksReport = { enabled: true, listening: false, agents: [] };
  }
  paintHooks();
}

/* ---- the orchestration pane ----
 *
 * Orca's `OrchestrationPane` (Settings-UurIK2fv.js:23256): one setup card for
 * the orchestration skill — a status pill, an install/update action, a
 * re-check, and a coverage strip that answers per agent. The rules live in
 * Rust (`zerocode_core::skill::orchestration_report`: what counts as the
 * skill, which roots each agent reads, the pill/chip asymmetry); this side
 * paints one report and opens one terminal.
 *
 * The install button does NOT run anything. It opens a real terminal tab
 * with the command TYPED and the Enter left to the person — Orca's inline
 * setup terminal makes the same promise in words ("Press Enter to run the
 * command."), and a settings button that silently executes npx would be a
 * different and worse feature than the one measured. */
let orchReport = null;
let orchLoading = false;
let orchInstalling = false;

/* In-app install of a skill this build carries. The person asked for the app
 * to carry its own copy: the npx road fetches from a repository not everyone
 * can reach and needs Node on PATH. The typed terminal road stays beside it
 * as the manual fallback. The outcome names each agent the file landed for,
 * and the one whose destination held somebody else's file. */
async function orchInstall() {
  if (orchInstalling || orchLoading || !orchReport) return;
  orchInstalling = true;
  paintOrchestration();
  await installBundledSkill("orchestration", el("orch-install-note"), refreshOrchestration);
  orchInstalling = false;
  paintOrchestration();
}

/* The command the card is currently about: update once installed, install
 * until then — Orca's `activeCommand`. */
function orchActiveCommand() {
  if (!orchReport) return "";
  return orchReport.installed ? orchReport.update_command : orchReport.install_command;
}

async function refreshOrchestration() {
  orchLoading = true;
  el("orch-error").hidden = true;
  paintOrchestration();
  try {
    orchReport = (await invoke("orchestration_report")) ?? null;
  } catch (error) {
    orchReport = null;
    const note = el("orch-error");
    note.textContent = String(error);
    note.hidden = false;
  }
  orchLoading = false;
  paintOrchestration();
}

/* Orca's `getAgentCoverageSummary`, sentence for sentence: checking, no
 * agents on PATH, full coverage, none, or the count of how far along it is. */
function orchSummary() {
  if (orchLoading || !orchReport) return t("orch.summary.checking", "설치된 에이전트와 스킬 경로를 확인하는 중…");
  const total = orchReport.agents.length;
  const ready = orchReport.agents.filter((row) => row.installed).length;
  if (total === 0) return t("orch.summary.none", "PATH에서 에이전트 CLI를 찾지 못했습니다. 에이전트를 설치한 뒤 다시 확인하세요.");
  if (ready === total) return t("orch.summary.full", "감지된 에이전트 {{total}}개 모두 스킬을 갖고 있습니다.", { total });
  if (ready === 0) return t("orch.summary.install", "위의 스킬을 설치한 뒤 다시 확인하세요.");
  return t("orch.summary.some", "감지된 에이전트 {{total}}개 중 {{ready}}개가 스킬을 갖고 있습니다.", { total, ready });
}

function paintOrchestration() {
  const pill = el("orch-pill");
  pill.classList.toggle("is-on", Boolean(orchReport?.installed));
  pill.classList.toggle("is-waiting", orchLoading);
  say(pill, () =>
    orchLoading
      ? t("orch.checking", "확인 중…")
      : orchReport?.installed
        ? t("orch.installed", "설치됨")
        : t("orch.missing", "설치 안 됨"),
  );
  say(el("orch-install-word"), () =>
    orchInstalling
      ? t("orch.installing", "설치 중…")
      : orchReport?.installed
        ? t("orch.update", "업데이트")
        : t("orch.install", "설치"),
  );
  // The command is language-neutral text; what changes is WHICH one.
  el("orch-command").textContent = orchActiveCommand();
  el("orch-install").disabled = orchLoading || orchInstalling || !orchReport;
  el("orch-install-terminal").disabled = orchLoading || orchInstalling || !orchReport || orchActiveCommand() === "";
  el("orch-recheck").disabled = orchLoading || orchInstalling;
  say(el("orch-summary"), orchSummary);
  // Chips only while coverage is partial — full coverage is the summary's
  // own sentence and a row of ten green chips under it says nothing more.
  const chips = el("orch-chips");
  chips.replaceChildren();
  const rows = orchReport?.agents ?? [];
  const partial = !orchLoading && rows.length > 0 && rows.some((row) => !row.installed);
  for (const row of partial ? rows : []) {
    const chip = document.createElement("span");
    chip.className = row.installed ? "orch-chip is-ready" : "orch-chip";
    const name = document.createElement("span");
    name.className = "orch-chip-name";
    name.textContent = row.label;
    const state = document.createElement("span");
    state.className = "orch-chip-state";
    state.textContent = row.installed ? t("orch.ready", "준비됨") : t("orch.notReady", "없음");
    chip.append(name, state);
    chips.appendChild(chip);
  }
}

/* Open a plain shell — never the default agent; npx wants a shell, not a
 * TUI — with the command typed and nothing pressed. Settings covers the
 * stage, so it steps aside first: a terminal opened behind the page it was
 * opened from is a button that appears to do nothing. */
async function orchOpenInstallTerminal() {
  const command = orchActiveCommand();
  if (command === "") return;
  try { await openSkillTerminal(command); } catch (error) { showError(error); }
}

el("orch-install").addEventListener("click", () => {
  void orchInstall();
});
el("orch-install-terminal").addEventListener("click", () => {
  void orchOpenInstallTerminal();
});
el("orch-recheck").addEventListener("click", () => {
  void refreshOrchestration();
});

/* 조율자가 시작한 에이전트를 어디에 담을지 — 그리고 그 답이 셋인 이유.
 *
 * 가운데 값이 진짜 상태이기 때문이다: `in-process`는 Claude 자신이 터미널
 * 없이 팀을 굴리는 방식이고, 셸에 가짜 `tmux`를 놓을 수 없는 기계가 정직하게
 * 할 수 있는 전부다. 켜진 것처럼 보이면서 판을 하나도 안 만드는 스위치보다
 * 그렇게 말하는 편이 낫다. 기본은 **판**: Orca는 꺼짐이 기본이지만
 * ("claudeAgentTeamsMode: off"), 사람이 화면 분할을 이름으로 두 번 청했다 —
 * 백엔드가 답의 주인이고, 여기 폴백은 그 답을 받지 못했을 때의 짐작이다. */
let agentTeamsMode = "panes";

async function refreshTeamsMode() {
  await refreshSettingsSnapshot({ reportError: true });
}

el("orch-teams-mode").addEventListener("change", (event) => {
  agentTeamsMode = event.target.value;
  void commitSetting("agent_teams_mode", "set_agent_teams_mode", { mode: agentTeamsMode });
});
el("orch-copy").addEventListener("click", () => {
  const command = orchActiveCommand();
  if (command !== "") void clipboardText.write(command);
});

/* ---- Computer Use ---------------------------------------------------------
 *
 * Two independent readiness facts share this page: macOS grants the signed
 * helper access to app interfaces/screenshots, and agent skill discovery says
 * which installed agents know the guarded `zerocode-computer` workflow. */
let computerUsePermissions = null;
let computerUseSkill = null;
let computerUseLoading = false;
let computerUseSkillLoading = false;
let computerUseInstalling = false;
let computerUseOperation = 0;

function computerUsePermissionState(id) {
  return computerUsePermissions?.permissions?.find((row) => row.id === id)?.status ?? null;
}

function computerUseSkillCommand() {
  if (!computerUseSkill) return "";
  return computerUseSkill.installed
    ? computerUseSkill.update_command
    : computerUseSkill.install_command;
}

async function refreshComputerUse() {
  const operation = ++computerUseOperation;
  computerUseLoading = true;
  computerUseSkillLoading = true;
  el("computer-use-permission-error").hidden = true;
  el("computer-use-skill-error").hidden = true;
  paintComputerUse();
  const [permissions, skill, guard] = await Promise.allSettled([
    invoke("computer_use_permission_status"),
    invoke("computer_use_skill_report"),
    invoke("computer_guard_status"),
  ]);
  if (operation !== computerUseOperation) return;
  computerGuard = guard.status === "fulfilled" && guard.value && typeof guard.value === "object" ? guard.value : null;
  paintComputerConfirm();
  if (permissions.status === "fulfilled") {
    computerUsePermissions = permissions.value;
  } else {
    computerUsePermissions = null;
    const error = el("computer-use-permission-error");
    error.textContent = String(permissions.reason);
    error.hidden = false;
  }
  if (skill.status === "fulfilled") {
    computerUseSkill = skill.value;
  } else {
    computerUseSkill = null;
    const error = el("computer-use-skill-error");
    error.textContent = String(skill.reason);
    error.hidden = false;
  }
  computerUseLoading = false;
  computerUseSkillLoading = false;
  paintComputerUse();
  // The Flow roster rides the same arrival: it reads the recipe folder and the
  // session folders, so it is spent when the pane is looked at, beside the
  // permission and skill reads above.
  void refreshFlows();
}

function computerUseCoverageSummary() {
  if (computerUseSkillLoading || !computerUseSkill) {
    return t("orch.summary.checking", "설치된 에이전트와 스킬 경로를 확인하는 중…");
  }
  const total = computerUseSkill.agents.length;
  const ready = computerUseSkill.agents.filter((row) => row.installed).length;
  if (total === 0) {
    return t("orch.summary.none", "PATH에서 에이전트 CLI를 찾지 못했습니다. 에이전트를 설치한 뒤 다시 확인하세요.");
  }
  if (ready === total) {
    return t("orch.summary.full", "감지된 에이전트 {{total}}개 모두 스킬을 갖고 있습니다.", { total });
  }
  if (ready === 0) return t("orch.summary.install", "위의 스킬을 설치한 뒤 다시 확인하세요.");
  return t("orch.summary.some", "감지된 에이전트 {{total}}개 중 {{ready}}개가 스킬을 갖고 있습니다.", { total, ready });
}

/* The last step is the person's (docs/design/computer-use-full-operator.md
 * §1.5): the three kinds the window asks about, on unless chosen off. */
const COMPUTER_CONFIRM_KINDS = ["payment", "transfer", "delete"];
let computerConfirmPolicy = { payment: true, transfer: true, delete: true };
let computerGuard = null;

function paintComputerConfirm() {
  for (const kind of COMPUTER_CONFIRM_KINDS) {
    el(`computer-confirm-${kind}`).checked = computerConfirmPolicy[kind] !== false;
  }
  const line = el("computer-guard-line");
  if (!computerGuard) {
    line.textContent = "";
    return;
  }
  line.textContent = computerGuard.stopped
    ? t("computerUse.guardStopped", "정지됨 ({{reason}}) · 비상 정지 {{hotkey}}", {
        reason: computerGuard.stopped,
        hotkey: computerGuard.hotkey ?? "",
      })
    : t("computerUse.guardLine", "비상 정지 {{hotkey}} · 이번 세션 행동 {{count}}", {
        hotkey: computerGuard.hotkey ?? "",
        count: computerGuard.actions ?? 0,
      });
}

for (const kind of COMPUTER_CONFIRM_KINDS) {
  el(`computer-confirm-${kind}`).addEventListener("change", () => {
    const on = el(`computer-confirm-${kind}`).checked;
    computerConfirmPolicy[kind] = on;
    void commitSetting(`computer_confirm_${kind}`, "set_computer_confirm", { kind, on });
  });
}

function paintComputerUse() {
  const states = COMPUTER_PERMISSION_IDS;
  const granted = states.filter((id) => computerUsePermissionState(id) === "granted").length;
  const allGranted = granted === states.length;
  const unavailable = computerUsePermissions?.helper_unavailable_reason ?? null;
  const checking = computerUseLoading && computerUsePermissions === null;
  // The report names its platform, and the platform decides what a grant
  // MEANS: macOS grants a signed helper two TCC permissions a person opens
  // System Settings for; Windows has no grant to give — UI Automation and
  // window capture run under this app's own rights — so there is nothing to
  // open and nothing to reset, and the copy must not send anyone looking.
  const platform = computerUsePermissions?.platform ?? null;
  const noSetupDoor = platform === "windows";
  say(el("computer-use-summary-title"), () =>
    checking
      ? t("computerUse.checking", "Computer Use 접근을 확인하는 중입니다.")
      : unavailable
        ? t("computerUse.unavailable", "Computer Use를 사용할 수 없습니다.")
        : allGranted
          ? t("computerUse.readyTitle", "Computer Use가 준비되었습니다.")
          : t("computerUse.finish", "로컬 앱을 사용하려면 설정을 마치세요."),
  );
  say(el("computer-use-summary-description"), () => {
    if (checking) {
      return t("computerUse.checkingAbout", "ZeroCode가 Computer Use 제공자의 접근 권한을 확인하고 있습니다.");
    }
    if (unavailable) return `${t("computerUse.unavailable", "Computer Use를 사용할 수 없습니다.")} ${unavailable}`;
    if (allGranted && noSetupDoor) {
      return t("computerUse.readyAboutWindows", "Windows에서는 별도 권한 부여 없이 UI 자동화와 창 캡처가 이 앱의 권한으로 동작합니다. 관리자 권한으로 실행된 앱은 조작할 수 없습니다.");
    }
    if (allGranted) return t("computerUse.readyAbout", "요청하면 에이전트가 앱 창을 검사하고 조작할 수 있습니다.");
    const count = states.length - granted;
    return count === 1
      ? t("computerUse.missingOne", "에이전트가 앱 창을 조작하려면 권한 1개가 필요합니다.")
      : t("computerUse.missingMany", "에이전트가 앱 창을 조작하려면 권한 {{count}}개가 필요합니다.", { count });
  });
  const readyPill = el("computer-use-ready-pill");
  readyPill.hidden = !allGranted;
  readyPill.classList.toggle("is-on", allGranted);
  say(readyPill, () => t("orch.ready", "준비됨"));
  el("computer-use-refresh").disabled = computerUseLoading;
  el("computer-use-refresh").classList.toggle("is-loading", computerUseLoading);

  for (const id of states) {
    const status = computerUsePermissionState(id);
    const node = el(`computer-use-${id}-state`);
    node.classList.toggle("is-on", status === "granted");
    say(node, () => {
      if (status === "granted") return t("computerUse.granted", "허용됨");
      if (status === "unsupported") return t("computerUse.unsupported", "지원되지 않는 플랫폼");
      return t("computerUse.notEnabled", "허용 안 됨");
    });
    const hint = el(`computer-use-${id}-hint`);
    hint.hidden = noSetupDoor || status === "granted" || status === "unsupported";
    say(hint, () => computerPermissionGuidance(id, computerUsePermissions?.helper_app_path));
    paintComputerUseTccRows(id);
    for (const action of ["open", "reset", "check"]) {
      const button = document.querySelector(`[data-permission-${action}="${id}"]`);
      button.disabled = computerUseLoading || (action !== "check" && (status === "unsupported" || unavailable !== null || noSetupDoor));
    }
  }
  const reset = el("computer-use-reset");
  reset.disabled = computerUseLoading || computerUsePermissions === null || unavailable !== null || noSetupDoor;
  say(reset, () => computerUseLoading
    ? t("computerUse.resetting", "접근 권한 초기화 중…")
    : t("computerUse.reset", "접근 권한 초기화"));

  const skillPill = el("computer-use-skill-pill");
  skillPill.classList.toggle("is-on", Boolean(computerUseSkill?.installed));
  say(skillPill, () => computerUseSkillLoading
    ? t("orch.checking", "확인 중…")
    : computerUseSkill?.installed
      ? t("orch.installed", "설치됨")
      : t("orch.missing", "설치 안 됨"));
  say(el("computer-use-skill-install-word"), () => computerUseInstalling
    ? t("orch.installing", "설치 중…")
    : computerUseSkill?.installed
      ? t("orch.update", "업데이트")
      : t("orch.install", "설치"));
  el("computer-use-skill-command").textContent = computerUseSkillCommand();
  el("computer-use-skill-install").disabled = computerUseSkillLoading || computerUseInstalling || !computerUseSkill;
  el("computer-use-skill-install-terminal").disabled = computerUseSkillLoading || computerUseInstalling || !computerUseSkill || computerUseSkillCommand() === "";
  el("computer-use-skill-recheck").disabled = computerUseSkillLoading || computerUseInstalling;
  say(el("computer-use-skill-summary"), computerUseCoverageSummary);
  const chips = el("computer-use-skill-chips");
  chips.replaceChildren();
  const agents = computerUseSkill?.agents ?? [];
  const partial = !computerUseSkillLoading && agents.length > 0 && agents.some((row) => !row.installed);
  for (const row of partial ? agents : []) {
    const chip = document.createElement("span");
    chip.className = row.installed ? "orch-chip is-ready" : "orch-chip";
    chip.innerHTML = '<span class="orch-chip-name"></span><span class="orch-chip-state"></span>';
    chip.querySelector(".orch-chip-name").textContent = row.label;
    chip.querySelector(".orch-chip-state").textContent = row.installed
      ? t("orch.ready", "준비됨")
      : t("orch.notReady", "없음");
    chips.appendChild(chip);
  }
}

/* The TCC rows under one permission (t-6058): both bundles' rows, the one the
 * permission is judged on first, each saying the grant the backend read and
 * offering only the buttons the row itself carries (`row.actions` — the core
 * table's, so only a stale row is ever reset from here). */
function paintComputerUseTccRows(id) {
  const list = el(`computer-use-${id}-tcc`);
  const rows = (computerUsePermissions?.tcc_rows ?? []).filter((row) => row.id === id);
  list.hidden = rows.length === 0;
  list.replaceChildren(...rows.map(computerUseTccRowNode));
}

function computerUseTccRowNode(row) {
  const item = document.createElement("li");
  item.className = "computer-use-tcc-row";
  item.dataset.grant = row.grant;
  item.dataset.bundle = row.bundle_id;
  item.dataset.judged = String(Boolean(row.judged));
  const name = document.createElement("span");
  name.className = "computer-use-tcc-name";
  name.textContent = row.name;
  item.append(name);
  if (row.judged) {
    const judged = document.createElement("span");
    judged.className = "computer-use-tcc-judged";
    say(judged, () => t("computerUse.tccJudged", "판정 행"));
    item.append(judged);
  }
  const words = document.createElement("span");
  words.className = "computer-use-tcc-grant";
  say(words, () => computerTccGrantWords(row));
  item.append(words);
  for (const action of row.actions ?? []) {
    const button = document.createElement("button");
    button.className = "btn";
    button.type = "button";
    button.dataset.tccAction = action;
    button.disabled = computerUseLoading;
    const word = COMPUTER_TCC_WORDS.action[action];
    say(button, () => (word ? t(word.key, word.word) : action));
    button.addEventListener("click", () => void computerUseCall(
      () => invoke("computer_use_tcc_row_action", { id: row.id, bundleId: row.bundle_id, action }),
      (report) => report,
    ));
    item.append(button);
  }
  return item;
}

/* One Computer Use call from this card: the card says it is busy, the newest
 * call's answer is the one kept, and a refusal lands in the card's own error
 * line. `take` folds the answer into the report the card paints. */
async function computerUseCall(ask, take) {
  const operation = ++computerUseOperation;
  computerUseLoading = true;
  el("computer-use-permission-error").hidden = true;
  paintComputerUse();
  try {
    const report = await ask();
    if (operation === computerUseOperation) computerUsePermissions = take(report);
  } catch (error) {
    if (operation !== computerUseOperation) return;
    const note = el("computer-use-permission-error");
    note.textContent = String(error);
    note.hidden = false;
  } finally {
    if (operation === computerUseOperation) {
      computerUseLoading = false;
      paintComputerUse();
    }
  }
}

async function computerUseOpenInstallTerminal() {
  const command = computerUseSkillCommand();
  if (command === "") return;
  try { await openSkillTerminal(command); } catch (error) { showError(error); }
}

el("computer-use-refresh").addEventListener("click", () => void refreshComputerUse());
function computerUseRequestPermission(id, reset) {
  return computerUseCall(
    () => invoke("open_computer_use_permission", { id, reset }),
    (report) => ({ ...computerUsePermissions, ...report }),
  );
}
for (const id of COMPUTER_PERMISSION_IDS) {
  document.querySelector(`[data-permission-open="${id}"]`).addEventListener("click", () => void computerUseRequestPermission(id, false));
  document.querySelector(`[data-permission-reset="${id}"]`).addEventListener("click", () => void computerUseRequestPermission(id, true));
  document.querySelector(`[data-permission-check="${id}"]`).addEventListener("click", () => void refreshComputerUse());
}
el("computer-use-reset").addEventListener("click", () => void computerUseCall(
  () => invoke("reset_computer_use_permissions"),
  (report) => report,
));
async function computerUseInstall() {
  if (computerUseInstalling || computerUseSkillLoading || !computerUseSkill) return;
  computerUseInstalling = true;
  paintComputerUse();
  await installBundledSkill("computer-use", el("computer-use-skill-install-note"), refreshComputerUse);
  computerUseInstalling = false;
  paintComputerUse();
}

el("computer-use-skill-install").addEventListener("click", () => void computerUseInstall());
el("computer-use-skill-install-terminal").addEventListener("click", () => void computerUseOpenInstallTerminal());
el("computer-use-skill-recheck").addEventListener("click", () => void refreshComputerUse());
el("computer-use-skill-copy").addEventListener("click", () => {
  const command = computerUseSkillCommand();
  if (command !== "") void clipboardText.write(command);
});
window.addEventListener("focus", () => {
  if (!settingsView.hidden && settingsPane === "computer-use" && !computerUseLoading) {
    void refreshComputerUse();
  }
});

/* ---- Flow cards (t-4260, docs/design/flow-engine-operator-and-qa.md §2.7) --
 *
 * A Flow is a recipe with `## Flow` / `## Checks` sections (core
 * `computer_flow`). `flow_list` hands back the roster, each Flow's last
 * verdict, and the two closed sets the controls draw from — the core enums
 * (`Policy::ALL`, `EvidenceLevel::ALL`), so the card never fixes a value of
 * its own. The card reads that and rewrites the two sections through
 * `flow_set`; running stays the existing `recipe-run` road, and the report and
 * document open where every artifact and file does. */
let flowData = null;
let flowSelected = null;
let flowLoading = false;

async function refreshFlows() {
  if (flowLoading) return;
  flowLoading = true;
  el("flow-error").hidden = true;
  try {
    flowData = await invoke("flow_list");
  } catch (error) {
    flowData = { flows: [], policies: [], levels: [] };
    const note = el("flow-error");
    note.textContent = String(error);
    note.hidden = false;
  } finally {
    flowLoading = false;
  }
  paintFlows();
}

function flowBySlug(slug) {
  return (flowData?.flows ?? []).find((flow) => flow.slug === slug) ?? null;
}

function paintFlows() {
  const card = el("flow-card");
  if (!flowData) { card.hidden = true; return; }
  card.hidden = false;
  const flows = flowData.flows ?? [];
  el("flow-empty").hidden = flows.length !== 0;
  // Keep a selection that still exists; otherwise the first Flow.
  if (!flowBySlug(flowSelected)) flowSelected = flows[0]?.slug ?? null;
  paintFlowRoster(flows);
  paintFlowDetail(flowBySlug(flowSelected));
}

function paintFlowRoster(flows) {
  const roster = el("flow-roster");
  roster.replaceChildren();
  for (const flow of flows) {
    const item = document.createElement("div");
    item.className = "flow-row";
    item.dataset.slug = flow.slug;
    if (flow.slug === flowSelected) item.setAttribute("aria-current", "true");
    const name = document.createElement("span");
    name.className = "flow-row-name";
    name.textContent = flow.name;
    const chips = document.createElement("span");
    chips.className = "flow-row-chips";
    for (const word of flowChipWords(flow)) {
      const chip = document.createElement("span");
      chip.className = "flow-chip";
      chip.textContent = word;
      chips.append(chip);
    }
    item.append(name, chips);
    // The shared helper wires the mouse AND the keyboard (role, tabindex,
    // Enter/Space), so a Flow is selectable without a pointer.
    actsAsButton(item, () => { flowSelected = flow.slug; paintFlows(); });
    roster.append(item);
  }
}

/* The roster's one-line read: where it is bound (hosts, else apps) and how many
 * checks — the site's own words, painted every time so the count follows the
 * locale. */
function flowChipWords(flow) {
  const where = (flow.fingerprint?.hosts?.length ? flow.fingerprint.hosts : flow.fingerprint?.apps) ?? [];
  const words = [...where, t("flow.checksChip", "체크 {{n}}", { n: flow.checks?.n ?? 0 })];
  if (flow.trigger) words.push(flow.trigger);
  return words;
}

function paintFlowDetail(flow) {
  const detail = el("flow-detail");
  if (!flow) { detail.hidden = true; return; }
  detail.hidden = false;
  el("flow-detail-name").textContent = flow.name;
  paintFlowVerdict(flow);
  el("flow-detail-summary").textContent = flowSummary(flow);
  paintFlowSegments(flow);
  paintFlowGuardedNote(flow);
  el("flow-report").hidden = !flow.last?.report;
}

function paintFlowVerdict(flow) {
  const badge = el("flow-verdict");
  badge.classList.remove("is-pass", "is-fail");
  const pass = flow.last?.pass;
  if (flow.last == null || pass == null) {
    badge.textContent = t("flow.noVerdict", "판정 없음");
  } else if (pass) {
    badge.textContent = t("flow.pass", "합격");
    badge.classList.add("is-pass");
  } else {
    badge.textContent = t("flow.fail", "불합격");
    badge.classList.add("is-fail");
  }
}

function flowSummary(flow) {
  const parts = [];
  const hosts = flow.fingerprint?.hosts ?? [];
  const apps = flow.fingerprint?.apps ?? [];
  if (hosts.length) parts.push(hosts.join(", "));
  if (apps.length) parts.push(apps.join(", "));
  parts.push(t("flow.checksSummary", "체크 {{n}}개 · 필수 {{required}}", {
    n: flow.checks?.n ?? 0,
    required: flow.checks?.required ?? 0,
  }));
  return parts.join(" · ");
}

/* The policy and evidence controls are drawn from the closed sets `flow_list`
 * hands back — the core enums — never a value fixed here. The chosen one wears
 * the window's own `.is-active`. */
function paintFlowSegments(flow) {
  buildFlowSegment(el("flow-policy-seg"), (flowData.policies ?? []).map((policy) => policy.value), flow.policy, "policy", flow.slug);
  buildFlowSegment(el("flow-evidence-seg"), flowData.levels ?? [], flow.evidence, "evidence", flow.slug);
}

function buildFlowSegment(seg, values, chosen, field, slug) {
  seg.replaceChildren();
  for (const value of values) {
    const button = document.createElement("button");
    button.type = "button";
    button.className = value === chosen ? "segment-btn is-active" : "segment-btn";
    button.dataset.value = value;
    button.setAttribute("aria-pressed", value === chosen ? "true" : "false");
    // A grammar word (dry, full, …): a technical token, shown as it is written.
    button.textContent = value;
    button.addEventListener("click", () => { if (value !== chosen) void flowSet(slug, { [field]: value }); });
    seg.append(button);
  }
}

/* When the chosen policy cannot run yet, the card says so in the enum's own
 * terms — the answer of `Policy::runnable`, which rides `flow_list` — and the
 * run button is held. */
function paintFlowGuardedNote(flow) {
  const note = el("flow-guarded-note");
  const option = (flowData.policies ?? []).find((policy) => policy.value === flow.policy);
  const blocked = Boolean(option && option.runnable && option.runnable.ok === false);
  el("flow-run").disabled = blocked;
  if (blocked) {
    note.textContent = t("flow.guardedUnbuilt", "이 빌드에서는 실행할 수 없습니다 — 돈 계약이 아직 서지 않았습니다.");
    note.hidden = false;
  } else {
    note.hidden = true;
  }
}

async function flowSet(slug, change) {
  el("flow-error").hidden = true;
  try {
    flowData = await invoke("flow_set", {
      slug,
      policy: change.policy ?? null,
      evidence: change.evidence ?? null,
    });
    flowSelected = slug;
    paintFlows();
  } catch (error) {
    const note = el("flow-error");
    note.textContent = String(error);
    note.hidden = false;
  }
}

/* The console submits recipe-run to the existing window dispatcher. */
async function flowRun() {
  const flow = flowBySlug(flowSelected);
  if (!flow) return;
  await flowLaunch(["recipe-run", "--name", flow.slug], flow.name);
}

el("flow-run").addEventListener("click", () => void flowRun());
el("flow-report").addEventListener("click", () => {
  const flow = flowBySlug(flowSelected);
  if (flow?.last?.report) void openBrowserTab(pathAsFileUrl(flow.last.report));
});
el("flow-open-doc").addEventListener("click", () => {
  const flow = flowBySlug(flowSelected);
  if (flow?.file) void openFile(flow.file, { preview: true });
});

/* Skills is a document view; Settings keeps a summary and its entry point. */
el("skills-open").addEventListener("click", openSkillsView);

function paintHooks() {
  el("hooks-enabled").checked = hooksReport.enabled !== false;
  // Only worth saying when hooks are ON: a bridge that never bound matters
  // exactly when somebody is expecting reports through it.
  el("hooks-quiet").hidden = !hooksReport.enabled || hooksReport.listening;
  const rows = hooksReport.agents ?? [];
  el("hooks-count").textContent = String(rows.filter((row) => row.state === "installed").length);
  el("hooks-install").disabled = !hooksReport.enabled;
  const host = el("hooks-list");
  host.replaceChildren();
  if (rows.length === 0) {
    const line = document.createElement("p");
    line.className = "agent-row agent-row-name";
    line.textContent = t("hooks.none", "훅을 심을 수 있는 에이전트가 없습니다");
    host.appendChild(line);
    return;
  }
  for (const row of rows) host.appendChild(hookRow(row));
}

/* One agent: its name, the file this window writes into, and one word for the
 * state. The file path is the whole point of the row — it is the thing a person
 * would want to go look at. */
function hookRow(row) {
  const line = document.createElement("div");
  line.className = "agent-row hook-row";
  const body = document.createElement("span");
  body.className = "agent-row-body";
  const name = document.createElement("span");
  name.className = "agent-row-name";
  name.textContent = agentName(row.agent);
  body.appendChild(name);
  const path = document.createElement("span");
  path.className = "agent-row-cmd";
  path.textContent = row.config_path ?? "";
  body.appendChild(path);
  // The reason, when there is one — an unreadable config is the case a person
  // has to be told about, because nothing was written and nothing will be.
  //
  // Under the path rather than beside it. A path is long and a reason is a
  // sentence; put both in the same row as the badge and they collide, which is
  // exactly what the first screenshot of this section showed.
  // Two kinds of reason, in the order a reader needs them: the named one first
  // (it says WHICH of the two homes this hook is in, which changes what the user
  // can expect), then whatever free text this window's Rust half computed.
  const note = hookNoteWord(row.note);
  if (note) {
    const named = document.createElement("span");
    named.className = "hook-why";
    named.textContent = note;
    body.appendChild(named);
  }
  if (row.detail) {
    const why = document.createElement("span");
    why.className = "hook-why";
    why.textContent = row.detail;
    body.appendChild(why);
  }
  line.appendChild(body);
  const word = document.createElement("span");
  word.className = "hook-state";
  word.dataset.state = row.state;
  word.textContent = hookStateWord(row.state);
  line.appendChild(word);
  return line;
}

/* Literal keys, not `t("hooks.state." + state)`: a computed key is invisible to
 * the catalog scanner, which then reports every one of them as unused and
 * cannot tell a missing translation from a dynamic one. */
function hookStateWord(state) {
  if (state === "installed") return t("hooks.installed", "심겨 있음");
  if (state === "partial") return t("hooks.partial", "일부만");
  if (state === "error") return t("hooks.error", "읽지 못함");
  return t("hooks.notInstalled", "없음");
}

/* The named reasons, mapped the same literal way and for the same reason: a
 * `t("hooks." + row.note)` would translate today and be reported as an unused
 * catalog entry forever. Returns "" for a row that carries no note, so the caller
 * asks one question instead of two. */
function hookNoteWord(note) {
  if (note === "mirror-only") {
    return t("hooks.mirrorOnly", "이 창이 만든 Codex 홈에서만 동작합니다");
  }
  if (note === "needs-trust") {
    return t("hooks.needsTrust", "썼지만 Codex가 아직 신뢰하지 않습니다 — 에이전트가 물어봅니다");
  }
  return "";
}

/* The agent's own name from the registry, falling back to its slug — the hook
 * list is built from a different table than the agent list, and a row that says
 * `command-code` when the rest of the window says `Command Code` reads as two
 * different products. */
function agentName(slug) {
  return agentRows.find((row) => row.id === slug)?.name ?? slug;
}

/* The mark and the working word an agent's own screen uses — `✻ Pondering…`
 * for Claude Code — ridden on its `list_agents` row from the core catalog's
 * one table (`zerocode_core::agent_voice`). Every agent the table does not
 * voice (Kimi, Grok, …) shares one mark and the window's own word, so the
 * page never invents a CLI's vocabulary and the rest stay uniform. */
function agentVoice(id) {
  const row = agentRows.find((one) => one.id === id);
  return {
    glyph: row?.glyph || "●",
    // The marks its spinner cycles through, in order; empty for a still mark.
    glyph_cycle: Array.isArray(row?.glyph_cycle) ? row.glyph_cycle : [],
    busy_word: row?.busy_word || t("worker.busy", "작업 중…"),
    // The verbs its spinner turns through while it works (t-6323 A4).
    spinner_verbs: Array.isArray(row?.spinner_verbs) ? row.spinner_verbs : [],
    // The tool it keeps its todo list with (t-6323 A7).
    todo_tool: row?.todo_tool ?? null,
  };
}

el("hooks-enabled").addEventListener("change", async (event) => {
  const wanted = event.target.checked;
  try {
    hooksReport = await invoke("set_hooks_enabled", { enabled: wanted });
  } catch (error) {
    showError(String(error));
    await refreshHooks();
    return;
  }
  paintHooks();
});

el("hooks-install").addEventListener("click", async () => {
  // Named for its element, not `button`: the scanner that stops a keyed element
  // being written behind its key matches by VARIABLE NAME across the whole file,
  // so a common local name inherits every other write that shares it.
  const hooksInstall = el("hooks-install");
  hooksInstall.disabled = true;
  try {
    hooksReport = await invoke("install_hooks");
  } catch (error) {
    showError(String(error));
  }
  paintHooks();
});

/* ---- more than one Claude account -----------------------------------------
 *
 * An account IS a config directory. `CLAUDE_CONFIG_DIR` decides which
 * credentials the CLI reads, so two directories are two logins and switching is
 * naming a different one on the next launch. That single fact is the whole
 * mechanism — see crates/zerocode-core/src/account.rs.
 *
 * This window never holds a token, and cannot: adding an account runs the CLI's
 * OWN login against a directory the backend made, and the one thing read back
 * out of it is who arrived. There is no endpoint here to log in against, no
 * token in the report, and nothing in this file that could leak one.
 *
 * Claude only. The other agents have no equivalent — a per-agent account list
 * would be four empty sections — so the backend answers with an empty
 * environment for every other id and this section names the one it applies to.
 */
let accountReport = { accounts: [], can_add: false };
/* A login in flight. One at a time, because the CLI's browser flow writes into
 * the directory it was given and a second login started on top of the first
 * would race it — and because the person is looking at a browser, not at this
 * window, so a second click is an accident rather than a second intent. */
let addingAccount = false;

async function refreshClaudeAccounts() {
  try {
    accountReport = (await invoke("claude_accounts")) ?? { accounts: [], can_add: false };
  } catch (error) {
    showError(error);
    accountReport = { accounts: [], can_add: false };
  }
  paintClaudeAccounts();
  void verifyClaudeAccounts();
}

/* The truth about the TOKENS, asked of the CLI after the rows are already
 * on screen. The store can only say who each directory names; four dead
 * logins looked perfectly connected until a terminal refused them (live
 * report 2026-08-14: "여러 계정이 연결됐음"). Rows repaint only when the
 * verdict changes something, and a report that was replaced while the ask
 * was in flight is left alone — the answer belongs to the rows it was
 * asked about. */
async function verifyClaudeAccounts() {
  const asked = accountReport;
  let verdicts;
  try {
    verdicts = (await invoke("verify_claude_accounts")) ?? [];
  } catch {
    return;
  }
  if (accountReport !== asked || verdicts.length === 0) return;
  let refreshed;
  try { refreshed = await invoke("claude_accounts"); } catch { return; }
  if (accountReport !== asked) return;
  accountReport = refreshed ?? asked;
  const dead = new Map(verdicts);
  let moved = accountReport !== asked;
  for (const row of accountReport.accounts ?? []) {
    // `false` is the CLI saying the login is gone. `null` is the probe not
    // being able to say — a CLI that would not start, a screen that was not
    // its JSON — and that is NOT evidence of an expiry: it clears the mark
    // the scan's guess put there rather than confirming it. Only an explicit
    // `false` puts 「로그인이 만료되었습니다」 under a row.
    const expired = row.signed_in && dead.get(row.id) === false;
    if (Boolean(row.login_expired) !== expired) {
      row.login_expired = expired;
      moved = true;
    }
  }
  if (moved) paintClaudeAccounts();
}

function paintClaudeAccounts() {
  const rows = accountReport.accounts ?? [];
  const count = el("account-count");
  const total = String(rows.length);
  if (count.textContent !== total) count.textContent = total;
  const host = el("account-list");
  const counts = new Map();
  for (const row of rows) {
    const key = claudeLoginKey(row);
    if (key) counts.set(key, (counts.get(key) ?? 0) + 1);
  }
  const shown = locale === "system" ? systemLocale : locale;
  /* Every row this paint wants, each with the shape it is drawn from. A row
   * whose shape did not move is the row already standing — kept, not rebuilt:
   * the usage answer repaints this list on every beat, and the same rows made
   * again were DOM mutations for nothing and rows whose identity was thrown
   * away (t-7538 E1). Its gauge line is written in place below, so a figure
   * or a countdown that moved rewrites that one line and nothing else. */
  const wanted = [
    // The original's 「시스템 기본값」, first and always — a ROW, not the
    // absence of one (설정 → AI 제공자 계정, above the managed list). It is
    // what makes the managed accounts 선택 사항: this machine's own Claude
    // login, with this window staying out of it entirely. Somebody who wants
    // that back should be able to click it rather than delete every account
    // to achieve it by subtraction.
    {
      key: "",
      shape: JSON.stringify([shown, !accountReport.active]),
      make: () => systemDefaultRow(),
    },
    ...rows.map((row) => {
      const duplicate = (counts.get(claudeLoginKey(row)) ?? 0) > 1;
      return {
        key: row.id,
        shape: JSON.stringify([
          shown, row, duplicate, row.id === accountReport.active,
          row.id === switchingAccount, addingAccount,
        ]),
        make: () => accountRow(row, duplicate),
      };
    }),
  ];
  const standing = new Map([...host.children].map((line) => [line.dataset.accountRow, line]));
  const lines = wanted.map(({ key, shape, make }) => {
    const held = standing.get(key);
    if (held && held.dataset.accountShape === shape) return held;
    const line = make();
    line.dataset.accountRow = key;
    line.dataset.accountShape = shape;
    return line;
  });
  const keep = new Set(lines);
  lines.forEach((line, at) => {
    const there = host.children[at] ?? null;
    if (there === line) return;
    if (there && !keep.has(there)) there.replaceWith(line);
    else host.insertBefore(line, there);
  });
  while (host.children.length > lines.length) host.lastElementChild.remove();
  for (const row of rows) {
    const gauge = host.querySelector(`.account-gauge[data-account="${CSS.escape(row.id)}"]`);
    const words = accountGaugeWords(row.id);
    if (gauge && gauge.textContent !== words) gauge.textContent = words;
  }
  paintAccountAdd();
  // Also recorded rather than written: which of the three sentences applies is
  // state, so a language change has to ask again rather than replay one of them.
  say(el("account-note"), () => accountNote(rows));
}

/* The button, and what it is allowed to say.
 *
 * Three states rather than two: a machine with no `claude` cannot log in at all,
 * and a login already running must not be started twice. Both are the same
 * disabled button wearing different words — the words are the part that stops
 * somebody clicking again. */
function paintAccountAdd() {
  // Named for its element rather than `add`: the scanner that stops a keyed
  // element from being written behind its key matches by VARIABLE NAME across
  // the whole file, so a common local shadows every other one that shares it.
  const accountAdd = el("account-add");
  const closed = addingAccount || !accountReport.can_add;
  if (accountAdd.disabled !== closed) accountAdd.disabled = closed;
  // Said through `say` rather than written straight in. The words depend on
  // state, so the key in the markup cannot produce them on its own — and a
  // language change redraws a keyed element FROM its key, which would put
  // 계정 추가 back over "signing in" while a browser flow was still open.
  say(accountAdd, () =>
    addingAccount
      ? t("settings.accounts.adding", "브라우저에서 로그인 중…")
      : t("settings.accounts.add", "계정 추가"),
  );
}

/* The sentence under the list. Says the thing the list cannot: why there is no
 * button, or what happens with nothing selected. */
function accountNote(rows) {
  if (!accountReport.can_add) {
    return t("settings.accounts.noClaude", "claude를 PATH에서 찾지 못해 계정을 추가할 수 없습니다");
  }
  if (rows.length === 0) {
    return t("settings.accounts.none", "추가된 계정이 없습니다. 지금까지 쓰던 로그인이 그대로 쓰입니다.");
  }
  return t("settings.accounts.hint", "고른 계정의 자격 증명으로 새 claude가 실행됩니다 — 즉시 적용됩니다. 이미 떠 있는 판은 제 로그인으로 계속하고, 한도 벽에 선 판만 같은 대화로 새 계정에서 이어집니다.");
}

/* One account: who it is, and the two things that can be done to it.
 *
 * The whole row selects, which is what a list of one-of-many choices should be —
 * so the only button is the destructive one. `aria-pressed` carries the
 * selection to anything not reading the check mark. */
/* This machine's own login, as a row you can choose.
 *
 * Chosen exactly when no managed account is — a state that used to be
 * unreachable while any account existed, because the backend fell back to the
 * first one and nothing on screen could say otherwise. That is why it needs a
 * name: the way back has to be a click, not the deletion of every account until
 * subtraction arrives at it. The second line says what choosing it means in the
 * plainest terms available: the app stops taking part.
 * Behind it `use_system_claude_login` clears the selection and writes nothing,
 * anywhere; every injury this feature has caused came from writing that home. */
function systemDefaultRow() {
  return ownLoginRow({
    // The report's `active` is an id the list holds or nothing at all — the
    // backend resolves it through `active_account` before answering — so the
    // absence of one IS this row, and no search through the rows is needed.
    chosen: !accountReport.active,
    name: t("settings.accounts.systemDefault", "시스템 기본값"),
    under: t(
      "settings.accounts.systemDefaultUnder",
      "이 기기의 Claude 로그인을 그대로 씁니다 — 이 창은 건드리지 않습니다",
    ),
    pick: () => pickSystemClaudeLogin(),
  });
}

/* 로그인이 움직인 뒤의 한 손 — 골랐든, 이 기계의 로그인으로 돌아갔든, 다시
 * 로그인했든, 지웠든. 목록을 다시 그리고 상태바의 수치를 **강제로** 다시 읽는다.
 * 수치는 로그인 하나의 사실인데, 백엔드의 5분 상한(`usage::MIN_REFETCH`)은
 * 강제가 아니면 옛 로그인의 답을 붙들고, 그 사이 상태바는 옛 수치를 든다. 전환
 * 뒤에도 한 자리의 수치가 그대로인 것이 「계정 체인지를 해도 체인지 안 되는 거
 * 같아」의 정체였다 — 전환은 다른 모든 곳에서 되고 있었다. 손마다 이 둘을 따로
 * 적었을 때 한 손(시스템 로그인)이 강제를 빠뜨렸다: `askEveryProviderUsage` 는
 * 인자를 받지 않는다. */
function claudeLoginMoved() {
  paintClaudeAccounts();
  void refreshClaudeUsage(true);
  void refreshClaudeAccountUsage(true);
  // The agent rows' 「로그인 필요」 was read off the login that just moved;
  // the backend forgot its snapshot, and this is the repaint that reads
  // the new one.
  void refreshAgents();
}

async function pickSystemClaudeLogin() {
  if (!accountReport.active) return;
  try {
    accountReport = await invoke("use_system_claude_login");
  } catch (error) {
    showError(String(error));
    await refreshClaudeAccounts();
    return;
  }
  claudeLoginMoved();
  sayUnrecordedPick();
}

/* 사람의 전환은 일어났는데 원장이 그 영수증을 거절했다면 — 성공처럼 조용히 넘기지 않고
 * 자동 전환과 같은 말로 알린다(astra R4). 영수증은 일지에 남아 다음 조회가 적는다. */
function sayUnrecordedPick() {
  const why = accountReport?.switch_unrecorded;
  if (!why) return;
  showError(t("settings.accounts.switchUnrecorded", "전환은 했지만 원장에 적지 못했습니다: {{why}}", { why }));
}

function claudeLoginKey(row) {
  const identity = row.pending ?? row;
  if (!identity.account_uuid?.trim() || !identity.organization_uuid?.trim()) return null;
  return JSON.stringify([identity.account_uuid.trim(), identity.organization_uuid.trim()]);
}

function accountRow(row, duplicate = false) {
  const line = document.createElement("div");
  line.className = "agent-row account-row";
  const chosen = row.id === accountReport.active;
  const pick = document.createElement("button");
  pick.className = "account-pick";
  pick.type = "button";
  pick.disabled = Boolean(row.pending);
  pick.setAttribute("aria-pressed", chosen ? "true" : "false");
  if (chosen) pick.classList.add("is-active");
  pick.appendChild(accountMark(chosen));
  const body = document.createElement("span");
  body.className = "agent-row-body";
  /* 이름과 배지는 **한 줄**이다. `agent-row-body` 는 세로 쌓기라서 배지를 그
   * 아래 넣으면 제 줄로 떨어진다 — 원본은 이름과 배지들을 한 flex 줄에 둔다
   * (`AccountsPane.tsx:1430`). 워크트리 카드의 `.wt-topline` 이 같은 모양이지만
   * 공유하지 않는다: 그 클래스에는 그 카드의 규칙들이 매달려 있고, 둘을 묶으면
   * 한쪽을 고칠 때마다 다른 쪽을 재야 한다. */
  const top = document.createElement("span");
  top.className = "account-topline";
  const who = document.createElement("span");
  who.className = "agent-row-name";
  who.textContent = row.email;
  top.appendChild(who);
  if (duplicate) {
    const badge = document.createElement("span");
    badge.className = "account-duplicate";
    badge.textContent = t("settings.accounts.sameLogin", "같은 로그인");
    top.appendChild(badge);
  }
  body.appendChild(top);
  const under = document.createElement("span");
  under.className = "agent-row-cmd";
  // 눌린 행은 그 자리에서 「전환 중…」을 입는다 — 라디오는 백엔드가 정말
  // 움직였을 때만 옮겨 가지만, 눌렀다는 사실은 지금 보여야 한다.
  if (row.id === switchingAccount) {
    line.classList.add("is-switching");
    under.textContent = t("settings.accounts.applying", "전환 중…");
  } else {
    under.textContent = accountUnder(row);
  }
  const alarm = paintAccountAlarm(line, accountAlarmed(row));
  if (alarm) top.appendChild(alarm);
  body.appendChild(under);
  // 이 계정의 제 게이지(t-7538) — 활성 계정만이 아니라 목록의 모든 계정이
  // 제 로그인으로 읽힌다. 읽은 적 없으면 그렇게 말한다.
  const gauge = document.createElement("span");
  gauge.className = "agent-row-cmd account-gauge";
  gauge.dataset.account = row.id;
  gauge.textContent = accountGaugeWords(row.id);
  body.appendChild(gauge);
  pick.appendChild(body);
  pick.addEventListener("click", () => pickClaudeAccount(row.id));
  line.appendChild(pick);
  /* The repair, on every row — which is the original's answer
   * (`AccountsPane.tsx:1490`: Re-authenticate and Remove stand on a row
   * whatever its state) and, measured against the report, the right one.
   *
   * It used to appear only where `login_expired` had been set, and that reads
   * as a smaller promise than it is: the verdict comes from a scan that has to
   * have RUN, so a person looking at a row the CLI has not been asked about
   * yet sees a healthy row with no repair on it — and the one road left is
   * 지우기 + 계정 추가, which throws away the directory, its settings and its
   * place in the list for what is only a dead credential (1-g15). A repair
   * offered before it is needed costs a button; a repair withheld until we are
   * sure costs the account. */
  if (row.pending) {
    line.classList.add("has-pending-identity");
    const choices = document.createElement("span");
    choices.className = "account-identity-choices";
    for (const [choice, label] of [
      ["add", t("settings.accounts.identityAdd", "새 계정으로 추가")],
      ["apply", t("settings.accounts.identityApply", "이 행에 적용")],
    ]) {
      const button = document.createElement("button");
      button.className = "account-identity-choice";
      button.type = "button";
      button.dataset.choice = choice;
      button.textContent = label;
      button.disabled = addingAccount;
      button.addEventListener("click", () => resolveClaudeIdentity(row, choice));
      choices.appendChild(button);
    }
    line.appendChild(choices);
  }
  const again = document.createElement("button");
  again.className = "account-relogin";
  again.type = "button";
  again.textContent = t("settings.accounts.relogin", "다시 로그인");
  again.addEventListener("click", () => reloginClaudeAccount(row, again));
  line.appendChild(again);
  const drop = document.createElement("button");
  drop.className = "account-drop";
  drop.type = "button";
  drop.textContent = t("settings.accounts.remove", "지우기");
  drop.addEventListener("click", () => dropClaudeAccount(row));
  line.appendChild(drop);
  return line;
}

/* The check on the chosen one — and a hollow ring on the rest, so the row that
 * is not chosen still shows WHERE the mark goes. A list where only one row has
 * a glyph reads as one row with a decoration. */
/* The machine's own login as a row — for either agent that holds accounts.
 *
 * One shape because it is one idea: a row that names no managed account and
 * is chosen exactly when none of them is. It carries no 지우기 — this login is
 * not ours to delete — and only the repairs its provider's own CLI offers,
 * handed in as `actions` by the caller that knows them: Codex has `login` and
 * `logout` verbs, so its row can offer both; Claude's system login is never
 * written by this window, so its row offers none ("every injury this feature
 * has caused came from writing that home"). The two callers differ only in
 * the words, in what a click means, and in those actions. */
function ownLoginRow({ chosen, name, under, pick: chose, alarmed = false, actions = [] }) {
  const line = document.createElement("div");
  line.className = "agent-row account-row";
  const pick = document.createElement("button");
  pick.className = "account-pick";
  pick.type = "button";
  pick.setAttribute("aria-pressed", chosen ? "true" : "false");
  if (chosen) pick.classList.add("is-active");
  pick.appendChild(accountMark(chosen));
  const body = document.createElement("span");
  body.className = "agent-row-body";
  const who = document.createElement("span");
  who.className = "agent-row-name";
  who.textContent = name;
  body.appendChild(who);
  const says = document.createElement("span");
  says.className = "agent-row-cmd";
  says.textContent = under;
  // 이 행에도 심각도가 온다 — 원본이 그렇게 한다(`AccountsPane.tsx:1306,1342`).
  // 행의 표시는 그 손이 남기고(부제의 색은 CSS 가 거기서 읽는다), 배지가 앉을
  // 이름 줄은 이 행의 모양을 아는 여기서 세운다.
  const alarm = paintAccountAlarm(line, alarmed);
  if (alarm) {
    // 이름이 이미 `body` 안에 있으므로 그 자리에 줄을 세우고 이름을 그 안으로
    // 옮긴다 — 새로 넣는 것이 아니라 **바꿔 끼우는** 것이라 순서가 그대로다.
    const top = document.createElement("span");
    top.className = "account-topline";
    who.replaceWith(top);
    top.append(who, alarm);
  }
  body.appendChild(says);
  pick.appendChild(body);
  pick.addEventListener("click", chose);
  line.appendChild(pick);
  for (const action of actions) {
    const button = document.createElement("button");
    button.className = action.className;
    button.type = "button";
    button.textContent = action.label;
    button.addEventListener("click", () => action.press(button));
    line.appendChild(button);
  }
  return line;
}

function accountMark(chosen) {
  const mark = document.createElement("span");
  mark.className = "account-mark";
  if (chosen) {
    const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
    svg.setAttribute("class", "icon");
    svg.setAttribute("aria-hidden", "true");
    const use = document.createElementNS("http://www.w3.org/2000/svg", "use");
    use.setAttribute("href", "#i-circle-check");
    svg.appendChild(use);
    mark.appendChild(svg);
  }
  return mark;
}

/* The second line: the organisation, or the reason this row cannot be used.
 *
 * A row whose directory lost its credentials stays in the list and says so.
 * Dropping it silently would leave a person who added an account with nothing
 * to click and no idea why. */
/* 이 행의 로그인이 지금 쓸 수 없는 상태인가 — 두 목록이 함께 쓰는 한 판정.
 *
 * 원본은 만료된 계정 행에 **심각도를 입힌다**: 파괴색 테두리와 옅은 파괴색 배경
 * (`AccountsPane.tsx:1405-1409`), 이름 옆의 파괴색 배지 'Needs re-auth'
 * (`:1451-1462`), 그리고 부제 줄 자체가 파괴색으로 바뀐다(`:1464`). 우리 행은
 * **문장 하나로만** 말하고 있었고, 이 요구를 낸 보고는 바로 그 문장을 보면서
 * 나왔다 — 목록을 훑는 사람은 문장이 아니라 색과 모양을 읽는다.
 *
 * 두 사실을 하나로 접는다: 자격 증명이 아예 없는 것과 있는데 죽은 것의 구별은
 * 부제가 말하고(`accountUnder`), 배지는 **심각도**만 말한다 — 원본의 배지도
 * 진단이 아니라 표식 하나다.
 *
 * 기록된 발산: 원본에서 이 심각도는 **Codex 행에만** 있다(`getCodexAccountAuthWarning`
 * 이 그쪽 오라클이므로 — Claude 쪽은 섹션 배너 하나로만 말한다, `:1189-1210`).
 * 우리는 두 provider 모두에 행마다 물을 수 있는 오라클을 가지고 있어서
 * (`verify_claude_accounts`/`verify_codex_accounts`) 둘 다 입는다. */
function accountAlarmed(row) {
  return !row.signed_in || Boolean(row.login_expired);
}

/* 심각도를 한 곳에서 켠다. 행에 표시만 남기고 **배지는 돌려준다**: 배지가 앉을
 * 이름 줄은 부르는 쪽이 알고, 부제의 색은 CSS 가 행의 표시로부터 읽는다
 * (`.account-row.is-alarmed .agent-row-cmd`) — 세 자리 중 둘을 한 판정에 매달아
 * 두는 것이 원본과 같은 모양이다. */
function paintAccountAlarm(line, alarmed) {
  if (!alarmed) return null;
  line.classList.add("is-alarmed");
  const badge = document.createElement("span");
  badge.className = "account-alarm";
  badge.textContent = t("settings.accounts.needsReauth", "다시 로그인 필요");
  return badge;
}

// One table owns the provider's organizationType words in every account row.
const CLAUDE_ORGANIZATION_TYPES = new Map([
  ["team", () => t("settings.accounts.typeTeam", "팀")],
  ["max", () => t("settings.accounts.typeMax", "맥스")],
  ["pro", () => t("settings.accounts.typePro", "프로")],
  ["free", () => t("settings.accounts.typeFree", "무료")],
]);

function claudeOrganizationLabel(identity) {
  const kind = identity.organization_type?.replace(/^claude_/, "");
  const type = CLAUDE_ORGANIZATION_TYPES.get(kind)?.() ?? "";
  return [identity.organization_name || identity.organization_uuid, type].filter(Boolean).join(" · ");
}

async function resolveClaudeIdentity(row, choice) {
  if (addingAccount) return;
  addingAccount = true;
  paintClaudeAccounts();
  try {
    accountReport = await invoke("resolve_claude_account_identity", { id: row.id, choice });
  } catch (error) {
    showError(String(error));
    await refreshClaudeAccounts();
  } finally {
    addingAccount = false;
    claudeLoginMoved();
  }
}

function accountUnder(row) {
  if (row.pending) {
    return t("settings.accounts.identityChanged", "로그인이 바뀌었습니다: {{old}} → {{new}}", {
      old: claudeOrganizationLabel(row), new: claudeOrganizationLabel(row.pending),
    });
  }
  if (!row.signed_in) {
    return t("settings.accounts.signedOut", "자격 증명이 없습니다 — 지우고 다시 추가하세요");
  }
  // A directory that can still name somebody whose TOKEN has died: the CLI
  // prints the cached identity in its banner and `Please run /login` in the
  // same breath, which is how "로그인이 되어 있는데 안 되어 있다고 나온다" was
  // reported (2026-08-14). Only a scan can know this, so it is said only when
  // one has run as this account.
  if (row.login_expired) {
    return t("settings.accounts.expired", "로그인이 만료되었습니다 — 다시 로그인하세요");
  }
  return claudeOrganizationLabel(row);
}

/* Log in again into the account's own directory — the repair keeps the row,
 * its settings and its place in the list, which 지우기 + 계정 추가 threw away
 * for what is only a credential problem (1-g15). */
async function reloginClaudeAccount(row, button) {
  if (addingAccount) return;
  addingAccount = true;
  button.disabled = true;
  paintAccountAdd();
  try {
    accountReport = await invoke("relogin_claude_account", { id: row.id });
  } catch (error) {
    showError(String(error));
    await refreshClaudeAccounts();
    return;
  } finally {
    addingAccount = false;
  }
  claudeLoginMoved();
}

/* The switch in flight, by the id it was asked for. The probe behind
 * `select_claude_account` starts the CLI twice and can take seconds; a click
 * that changes nothing on screen for that long reads as a click that did not
 * take ("리얼타임으로 반영"). So the row wears 「전환 중…」 the moment it is
 * pressed — honest, because the radio itself only moves when the backend has
 * really moved. */
let switchingAccount = null;

async function pickClaudeAccount(id) {
  if (id === accountReport.active || switchingAccount !== null) return;
  switchingAccount = id;
  paintClaudeAccounts();
  try {
    accountReport = await invoke("select_claude_account", { id });
  } catch (error) {
    showError(String(error));
    switchingAccount = null;
    await refreshClaudeAccounts();
    return;
  }
  switchingAccount = null;
  claudeLoginMoved();
  sayUnrecordedPick();
  // And nothing else: the panes already running claude keep the login they
  // were started with (see the block below). A person who wants a pane on
  // the new account opens a new one.
}

/* ---- 계정 전환과 산 판 ----
 *
 * 전환의 나머지 반쪽은 이제 백엔드의 것이다(t-7538). 새로 여는 터미널은 이미
 * 새 계정이다 — `launch_env_for`가 launch마다 materialize한다. 이미 떠 있는
 * claude 판은 **건드리지 않는다**: 일하는 판도, 쉬는 판도, 제 로그인(계정별
 * 키체인 항목)으로 계속한다. 옛 길은 전환 순간 모든 claude 판을 큐에 넣고
 * `done`에 닿는 숨마다 `--resume`으로 다시 세우며 옛 셸을 닫았는데, 그 닫힘이
 * 원장에 `worker_died`로 닿아 워커 다섯이 「끝남」이 되고 새 자리를 만들어야
 * 했다(09-24 21:2x, 함정 410). 한도 벽에 선 판만 움직이고, 그것도 원장이
 * 워커를 먼저 재운 뒤 같은 worker id·dispatch로 다시 앉히는 백엔드의
 * `claude_autoswitch_apply`가 한다 — 창은 그 결과를 그리기만 한다. */

/* 계정별 사용량과 자동 전환 계획(t-7538): `claude_account_usage`의 답을 들고
 * 계정 목록의 게이지 칸, 아래의 제안 줄, 상태 바의 「다음」 낱말을 그린다.
 * 판정은 전부 백엔드의 표(`CLAUDE_ACCOUNT_AUTOSWITCH`)가 하고 — 다음 후보도
 * `plan.next`로 온다 — 여기서는 낱말만 고른다. */
let accountUsageReport = null;
let claudeAutoSwitchMode = "ask";
let applyingAccountSwitch = false;
/* The token the window last applied: the same plan on the next beat is
 * not applied again from here — the backend re-plans and would refuse it
 * too, but a second call for one decision is a second call. */
let appliedSwitchToken = null;
const accountUsageAskTimer = { handle: null, outAt: null };

function accountUsageRow(id) {
  return accountUsageReport?.accounts?.find((row) => row.id === id) ?? null;
}

function accountFitness(id) {
  return accountUsageReport?.plan?.fitness?.find((row) => row.id === id) ?? null;
}

/* 새 표면(상태 바·제안 줄·토스트)이 계정을 부르는 이름 — 요금제 낱말과 계정 id.
 * 이메일은 사람이 관리하는 계정 목록 줄에만 있다(t-7538: 토큰·이메일은 로그·
 * UI·원장에 싣지 않는다). */
function accountBadge(id) {
  const kind = (
    accountUsageRow(id)?.organization_type ??
    accountReport.accounts?.find((row) => row.id === id)?.organization_type
  )?.replace(/^claude_/, "");
  const type = CLAUDE_ORGANIZATION_TYPES.get(kind)?.() ?? "";
  return [type, id].filter(Boolean).join(" ");
}

/* 한 계정의 게이지 낱말: 창마다 「이름 N% · 리셋까지」, 뒤에 표의 판정(남은 몫·
 * 한도·읽지 못함·오래됨·거절). 숫자는 백엔드의 것이고 여기서는 낱말만 고른다. */
function accountGaugeWords(id) {
  const held = accountUsageRow(id);
  const fit = accountFitness(id);
  const usage = held?.usage ?? null;
  const parts = [];
  for (const [key, label] of [
    ["session", t("settings.accounts.gaugeSession", "세션")],
    ["weekly", t("settings.accounts.gaugeWeekly", "주")],
    ["fable_weekly", t("settings.accounts.gaugeFable", "Fable")],
  ]) {
    const window = usage?.[key];
    if (!window) continue;
    const reset = window.resets_at == null ? null : statusUsageDuration(window.resets_at - Date.now());
    parts.push(reset ? `${label} ${Math.round(window.used_percent)}% (${reset})` : `${label} ${Math.round(window.used_percent)}%`);
  }
  if (usage?.status === "denied") parts.push(t("settings.accounts.gaugeDenied", "키체인 거절 — 눌러서 다시"));
  else if (usage?.status === "signed_out") parts.push(t("settings.accounts.gaugeSignedOut", "로그인 없음"));
  else if (fit?.unfit === "blocked") parts.push(t("settings.accounts.gaugeBlocked", "한도"));
  else if (fit?.unfit === "stale") parts.push(t("settings.accounts.gaugeStale", "오래됨"));
  else if (fit?.unfit || !usage) parts.push(t("settings.accounts.gaugeUnknown", "읽지 못함"));
  else if (typeof fit?.room_percent === "number") {
    parts.push(t("settings.accounts.gaugeRoom", "남은 몫 {{room}}%", { room: fit.room_percent }));
  }
  if (held?.fetching) parts.push("…");
  return parts.join(" · ");
}

/* 계획이 옮길 벽 판. */
function accountSwitchMoves(plan) {
  return (plan?.walled ?? []).filter((row) => row.verdict?.kind === "switch");
}

/* 제안 줄 — `ask`에서 표가 「바꾸자」거나 벽 판을 옮기자고 할 때만 서고, 단추는
 * 바로 그 계획의 토큰으로만 적용한다. 늦은 「예」는 백엔드가 토큰 비교로
 * 거절한다. `wait`는 어느 모드에서든 그 분(分)을 말하고 단추를 내놓지 않는다. */
function paintAccountSwitchNote() {
  const note = el("account-switch-note");
  const plan = accountUsageReport?.plan ?? null;
  const decision = plan?.decision ?? null;
  const said = el("account-switch-said");
  const button = el("account-switch-now");
  // Only what moved is written: the note repaints on every usage answer
  // (t-7538 E1).
  const paint = (shown, offered, words = "") => {
    if (note.hidden !== !shown) note.hidden = !shown;
    if (!shown) return;
    if (button.hidden !== !offered) button.hidden = !offered;
    if (said.textContent !== words) said.textContent = words;
    if (!offered) return;
    if (button.disabled !== applyingAccountSwitch) button.disabled = applyingAccountSwitch;
    // The button applies the plan whose words stand beside it, and no
    // other (astra R1): its token is the one this paint read.
    if (button.dataset.token !== plan.token) button.dataset.token = plan.token;
  };
  if (!plan || !decision || plan.mode === "off") {
    paint(false, false);
    return;
  }
  if (decision.kind === "wait") {
    paint(true, false, t("settings.accounts.switchWait", "리셋까지 {{minutes}}분 — 기다립니다", {
      minutes: decision.minutes,
    }));
    return;
  }
  const moves = accountSwitchMoves(plan);
  if (plan.mode !== "ask" || (decision.kind !== "switch" && moves.length === 0)) {
    paint(false, false);
    return;
  }
  paint(true, true, accountSwitchProposal(plan));
}

/* 제안 한 문장 — 설정 화면의 줄과 알림이 같은 말을 한다. */
function accountSwitchProposal(plan) {
  const decision = plan.decision;
  const moves = accountSwitchMoves(plan);
  const to = decision.kind === "switch" ? decision.to : plan.landing;
  const parts = [
    decision.kind === "switch"
      ? t("settings.accounts.switchAsk", "계정을 {{to}}(으)로 바꿀까요? {{why}}", {
          to: accountBadge(to),
          why: switchReasonWords(decision.reason),
        })
      : t("settings.accounts.switchAskPanes", "벽에 선 판을 {{to}}(으)로 옮길까요?", { to: accountBadge(to) }),
  ];
  if (moves.length > 0) {
    parts.push(t("settings.accounts.switchPanes", "벽에 선 판 {{count}}개는 같은 대화로 이어집니다", { count: moves.length }));
  }
  return parts.join(" ");
}

/* `ask`의 알림 — 상태 바를 보는 사람에게 같은 한 줄과 단추를, 제안(토큰)마다 한 번.
 * 같은 제안에 다시 뜨지 않고, 단추는 **그 알림을 만든 제안의 토큰**으로만 적용한다
 * (astra R1): 누를 때의 최신 계획을 읽지 않는다. 제안이 바뀌거나 사라지면 옛 알림은
 * 걷고, 걷히기 전에 눌린 옛 단추는 새 계획을 고르지 못한다. 설정 화면의 줄과 상태
 * 바의 「확인 기다림」은 알림이 사라진 뒤에도 남는다. */
let proposedSwitchToken = null;
let proposedSwitchNote = null;

function offerAccountSwitch() {
  const plan = accountUsageReport?.plan ?? null;
  const decision = plan?.decision ?? null;
  const proposes =
    plan?.mode === "ask" &&
    (decision?.kind === "switch" || accountSwitchMoves(plan).length > 0);
  if (proposedSwitchNote && (!proposes || plan.token !== proposedSwitchToken)) {
    proposedSwitchNote.remove();
    proposedSwitchNote = null;
  }
  if (!proposes) {
    proposedSwitchToken = null;
    return;
  }
  if (applyingAccountSwitch || plan.token === proposedSwitchToken) return;
  const token = plan.token;
  proposedSwitchToken = token;
  proposedSwitchNote = toast(accountSwitchProposal(plan), "", {
    action: {
      label: t("settings.accounts.switchNow", "지금 바꾸기"),
      run: () => void applyAccountSwitch("ask", token),
    },
  });
}

function switchReasonWords(reason) {
  if (!reason) return "";
  if (reason === "walled") return t("settings.accounts.reasonWalled", "판이 한도 벽에 섰습니다");
  return t("settings.accounts.reasonNearLimit", "{{window}} {{used}}% 사용", {
    window: reason.near_limit?.window ?? "",
    used: reason.near_limit?.used_percent ?? "",
  });
}

/* 상태 바: 활성 조각 옆의 「다음 계정 · 남은 몫 · 자동 전환 낱말」. 표의 판정이
 * 숫자를 못 내는 때(후보 없음·쉼·읽지 못함·리셋 대기·확인 기다림)는 그 낱말을
 * 말한다 — 0%나 빈칸으로 감추지 않는다. */
function paintClaudeNext() {
  const next = document.getElementById("sb-claude-next");
  if (!next) return;
  const plan = accountUsageReport?.plan ?? null;
  if (!plan || (plan.fitness ?? []).length < 2) {
    if (!next.hidden) next.hidden = true;
    return;
  }
  const decision = plan.decision ?? {};
  const moves = accountSwitchMoves(plan);
  const parts = [];
  if (decision.kind === "wait") {
    parts.push(t("usage.nextWait", "리셋 {{minutes}}분", { minutes: decision.minutes }));
  } else if (decision.kind === "switch") {
    parts.push(t("usage.nextSwitch", "→ {{account}} {{room}}%", {
      account: accountBadge(decision.to),
      room: accountFitness(decision.to)?.room_percent ?? "?",
    }));
  } else if (decision.why === "unread") {
    parts.push(t("usage.nextUnread", "사용량 읽지 못함"));
  } else if (plan.next) {
    parts.push(t("usage.nextAccount", "다음 {{account}} {{room}}%", {
      account: accountBadge(plan.next.id),
      room: plan.next.room_percent,
    }));
  } else {
    parts.push(t("usage.nextNone", "다음 계정 없음"));
  }
  if (decision.why === "cooldown" && plan.cooldown_until_ms) {
    parts.push(t("usage.nextCooldown", "전환 쉼 {{minutes}}분", {
      minutes: Math.max(1, Math.ceil((plan.cooldown_until_ms - Date.now()) / 60000)),
    }));
  }
  if (moves.length > 0) parts.push(t("usage.nextWalled", "벽 판 {{count}}", { count: moves.length }));
  parts.push({
    off: t("settings.accounts.autoSwitchOff", "끔"),
    ask: t("settings.accounts.autoSwitchAsk", "물어보기"),
    auto: t("settings.accounts.autoSwitchAuto", "자동"),
  }[plan.mode] ?? plan.mode);
  if (plan.mode === "ask" && (decision.kind === "switch" || moves.length > 0)) {
    parts.push(t("usage.nextAsk", "확인 기다림"));
  }
  const words = parts.join(" · ");
  if (next.textContent !== words) next.textContent = words;
  if (next.hidden) next.hidden = false;
}

async function refreshClaudeAccountUsage(force = false) {
  let report;
  try {
    report = await invoke("claude_account_usage", { force });
  } catch (error) {
    showError(error);
    return;
  }
  accountUsageReport = report ?? null;
  claudeAutoSwitchMode = report?.plan?.mode ?? claudeAutoSwitchMode;
  paintClaudeAccounts();
  paintAccountSwitchNote();
  paintClaudeNext();
  offerAccountSwitch();
  // 읽기가 나가 있는 동안은 다시 묻는다 — 백엔드의 마루가 물음을 공짜로 만든다.
  clearTimeout(accountUsageAskTimer.handle);
  if (report?.fetching) {
    if (force || accountUsageAskTimer.outAt === null) accountUsageAskTimer.outAt = Date.now();
    accountUsageAskTimer.handle = setTimeout(
      () => refreshClaudeAccountUsage(false),
      usageFollowUpMs(Date.now() - accountUsageAskTimer.outAt),
    );
  } else {
    accountUsageAskTimer.outAt = null;
  }
  // `auto`: 표가 바꾸자면(기본 계정이든 벽 판이든) 바로 — 그 계획의 토큰으로. 한 번에 하나.
  const plan = report?.plan;
  if (plan?.mode === "auto" && (plan.decision?.kind === "switch" || accountSwitchMoves(plan).length > 0)) {
    void applyAccountSwitch("auto", plan.token);
  }
}

/* 적용은 늘 **어느 제안의** 토큰으로 한다(astra R1) — 알림 단추는 그 알림의 토큰,
 * 설정 줄의 단추는 그 줄을 그린 계획의 토큰, `auto`는 그 박자의 계획의 토큰. 창이
 * 이미 다른 계획을 들고 있으면 그 옛 토큰은 보내지도 않고 「제안이 바뀌었다」고
 * 말한다. 보낸 토큰도 백엔드가 지금 계획과 다시 대조해 다르면 거절한다. */
async function applyAccountSwitch(by, token) {
  if (!token || applyingAccountSwitch || token === appliedSwitchToken) return;
  if (token !== accountUsageReport?.plan?.token) {
    showError(t("settings.accounts.switchStale", "제안이 바뀌어 그 제안으로는 바꾸지 않았습니다 — 지금 제안을 확인하세요"));
    return;
  }
  applyingAccountSwitch = true;
  appliedSwitchToken = token;
  paintAccountSwitchNote();
  let applied;
  try {
    applied = await invoke("claude_autoswitch_apply", { token, by });
  } catch (error) {
    showError(error);
    applyingAccountSwitch = false;
    await refreshClaudeAccounts();
    return;
  } finally {
    applyingAccountSwitch = false;
  }
  try {
    accountReport = await invoke("claude_accounts");
  } catch {
    // The report is re-read below anyway.
  }
  claudeLoginMoved();
  // 원장이 영수증을 거절했으면 성공처럼 말하지 않는다 — 전환은 일어났고, 그 사실을
  // 오류로 알린다.
  if (applied?.receipt_error) {
    showError(t("settings.accounts.switchUnrecorded", "전환은 했지만 원장에 적지 못했습니다: {{why}}", {
      why: applied.receipt_error,
    }));
    return;
  }
  const moved = (applied?.panes ?? []).filter((pane) => pane.ok).length;
  const stuck = (applied?.panes ?? []).length - moved;
  const words = [
    applied?.switched_default
      ? t("settings.accounts.switched", "계정을 {{to}}(으)로 바꿨습니다", { to: accountBadge(applied?.to ?? "") })
      : t("settings.accounts.switchedPanes", "벽에 선 판을 {{to}}(으)로 옮겼습니다", { to: accountBadge(applied?.to ?? "") }),
    t("settings.accounts.switchedMoved", "판 {{count}}개 이어짐", { count: moved }),
  ];
  if (stuck > 0) words.push(t("settings.accounts.switchedStuck", "{{count}}개는 그대로", { count: stuck }));
  toast(words.join(" · "));
}

el("account-switch-now").addEventListener("click", () =>
  void applyAccountSwitch("ask", el("account-switch-now").dataset.token));
el("account-autoswitch").addEventListener("change", (event) => {
  claudeAutoSwitchMode = event.target.value;
  void commitSetting("claude_autoswitch_mode", "set_claude_autoswitch_mode", {
    mode: claudeAutoSwitchMode,
  });
});

/* Removing an account deletes its credentials with it, so it asks first.
 *
 * Anything but a plain yes leaves the account alone — the dialog's third answer
 * and its dismissal both mean "not this". */
async function dropClaudeAccount(row) {
  const said = await askConfirm({
    title: t("settings.accounts.removeAsk", "이 계정을 지울까요?"),
    body: t("settings.accounts.removeBody", "{{who}}의 로그인이 이 기계에서 지워집니다.", {
      who: row.label,
    }),
    confirm: t("settings.accounts.remove", "지우기"),
    deny: t("settings.accounts.keep", "그대로 두기"),
    danger: true,
  });
  if (said !== true) return;
  try {
    accountReport = await invoke("remove_claude_account", { id: row.id });
  } catch (error) {
    showError(String(error));
    await refreshClaudeAccounts();
    return;
  }
  // Removing the chosen account moves the choice to another row, and the bar's
  // figure with it.
  claudeLoginMoved();
}

/* Add an account: the CLI's login, in front of the person, in a browser.
 *
 * It can take minutes and it is not this window doing the work, so the button
 * says what is happening and stays disabled until it is over. The report is
 * re-read rather than trusted from the call, because a login the person
 * abandoned leaves the list exactly as it was. */
el("account-add").addEventListener("click", async () => {
  if (addingAccount) return;
  addingAccount = true;
  paintAccountAdd();
  try {
    accountReport = await invoke("add_claude_account");
  } catch (error) {
    showError(String(error));
  } finally {
    addingAccount = false;
  }
  paintClaudeAccounts();
});

/* ---- the same for Codex, by its own mechanism ----
 *
 * `CODEX_HOME` rather than `CLAUDE_CONFIG_DIR`, and the variable names a whole
 * HOME: a managed one inherits the person's `config.toml` and `AGENTS.md` so
 * their settings survive the switch, and never `auth.json`, which is the one
 * file that must differ. Orca does the same (`syncCanonicalConfigIntoManagedHome`,
 * out/main/index.js:216018).
 *
 * The list has one row the Claude list does not: the machine's own login. For
 * Codex that is a real choice — Orca keeps `activeAccountId: null` as a state
 * and reads `~/.codex` live to say whose it is (:215812) — so it is drawn as a
 * row a person can pick, not as the absence of one. */
let codexAccountReport = { accounts: [], can_add: false };
let addingCodexAccount = false;

async function refreshCodexAccounts() {
  try {
    codexAccountReport = (await invoke("codex_account_list")) ?? { accounts: [], can_add: false };
  } catch (error) {
    showError(error);
    codexAccountReport = { accounts: [], can_add: false };
  }
  paintCodexAccounts();
  void verifyCodexAccounts();
}

/* The truth about the TOKENS, asked of the CLI after the rows are on screen.
 * The Claude pair is `verifyClaudeAccounts` and this is the same shape for the
 * same reason: the store only knows whether `auth.json` has something in it,
 * and a token that has died leaves that file exactly as it was — so a dead
 * account read as a healthy row with a workspace label on it. Rows repaint
 * only when the verdict changes something, and a report replaced while the ask
 * was in flight is left alone: the answer belongs to the rows it was asked
 * about. */
async function verifyCodexAccounts() {
  const asked = codexAccountReport;
  let verdicts;
  try {
    verdicts = (await invoke("verify_codex_accounts")) ?? [];
  } catch {
    return;
  }
  if (codexAccountReport !== asked || verdicts.length === 0) return;
  const dead = new Map(verdicts);
  let moved = false;
  for (const row of codexAccountReport.accounts ?? []) {
    // Both facts, not one: the store must say there ARE credentials and the
    // CLI must say they no longer work. A missing `codex` answers false for
    // everybody, and that must not condemn an account that never had a login.
    const expired = row.signed_in && dead.get(row.id) === false;
    if (Boolean(row.login_expired) !== expired) {
      row.login_expired = expired;
      moved = true;
    }
  }
  if (moved) paintCodexAccounts();
}

function paintCodexAccounts() {
  const rows = codexAccountReport.accounts ?? [];
  el("codex-account-count").textContent = String(rows.length);
  const host = el("codex-account-list");
  host.replaceChildren();
  // The machine's own login first, always — it is what runs when nothing is
  // chosen, and a list that only shows the managed homes cannot say that.
  host.appendChild(codexSystemRow());
  for (const row of rows) host.appendChild(codexAccountRow(row));
  paintCodexAccountAdd();
  say(el("codex-account-note"), () => codexAccountNote(rows));
}

function paintCodexAccountAdd() {
  const codexAdd = el("codex-account-add");
  codexAdd.disabled = addingCodexAccount || !codexAccountReport.can_add;
  say(codexAdd, () =>
    addingCodexAccount
      ? t("settings.codexAccounts.adding", "브라우저에서 로그인 중…")
      : t("settings.codexAccounts.add", "계정 추가"),
  );
}

function codexAccountNote(rows) {
  if (!codexAccountReport.can_add) {
    return t("settings.codexAccounts.noCodex", "codex를 PATH에서 찾지 못해 계정을 추가할 수 없습니다");
  }
  if (rows.length === 0) {
    return t("settings.codexAccounts.none", "추가된 계정이 없습니다. 이 기계의 로그인이 그대로 쓰입니다.");
  }
  return t("settings.codexAccounts.hint", "고른 계정의 CODEX_HOME으로 codex가 실행됩니다 — 새로 여는 터미널부터. 이미 열린 창은 그대로입니다.");
}

/* The machine's own login as a row. Named when it can be named, and honest
 * about a key login having no name to give. */
function codexSystemRow() {
  return ownLoginRow({
    chosen: !codexAccountReport.active,
    name: t("settings.codexAccounts.system", "이 기계의 로그인"),
    under: codexSystemUnder(),
    alarmed: codexSystemSignedOut(),
    pick: () => pickCodexAccount(null),
    // The CLI's own two verbs for its own home, and nothing more: a dead
    // `~/.codex` token used to have no road out of this pane at all
    // ("gpt쪽은 다시 로그인 지우기가 없어").
    actions: [
      {
        className: "account-relogin",
        label: t("settings.accounts.relogin", "다시 로그인"),
        press: (button) => reloginCodexLogin(button),
      },
      {
        className: "account-logout",
        label: t("settings.accounts.logout", "로그아웃"),
        press: () => logoutCodexLogin(),
      },
    ],
  });
}

/* The Claude pair is `claudeLoginMoved`, for the same reason: the bar's Codex
 * figure is a fact about ONE login, and without a forced re-read the segment
 * keeps the previous login's word — 「로그인 필요」 standing under an account
 * that is signed in — until an agent stirs or the fifteen-minute poll comes
 * round (live report 2026-09-02: 「계정 전환시 실시간연동안됨」). The login
 * verbs below all wait for the CLI's browser round trip before they answer, so
 * by the time this runs the new login is on disk. */
function codexLoginMoved() {
  paintCodexAccounts();
  void refreshCodexUsage(true);
  void refreshAgents();
}

/* Log the machine's own login in again — the same road `reloginCodexAccount`
 * walks for a managed home, on `~/.codex`, and the same one-at-a-time guard:
 * two browser logins racing for one credential file is the mistake the guard
 * exists to prevent, whichever home they aim at. */
async function reloginCodexLogin(button) {
  if (addingCodexAccount) return;
  addingCodexAccount = true;
  button.disabled = true;
  paintCodexAccounts();
  try {
    codexAccountReport = await invoke("relogin_codex_login");
  } catch (error) {
    showError(String(error));
    await refreshCodexAccounts();
    return;
  } finally {
    addingCodexAccount = false;
  }
  codexLoginMoved();
}

/* Log the machine's own login out. Asked first, the way 지우기 asks: the
 * credential this removes is the one every terminal without a chosen account
 * runs on, and the managed homes stay exactly as they are. */
async function logoutCodexLogin() {
  const said = await askConfirm({
    title: t("settings.accounts.logoutAsk", "이 기계의 로그인을 로그아웃할까요?"),
    body: t(
      "settings.accounts.logoutBody",
      "~/.codex의 자격 증명이 지워집니다. 추가한 계정들은 그대로입니다.",
    ),
    confirm: t("settings.accounts.logout", "로그아웃"),
    deny: t("settings.accounts.keep", "그대로 두기"),
    danger: true,
  });
  if (said !== true) return;
  try {
    codexAccountReport = await invoke("logout_codex_login");
  } catch (error) {
    showError(String(error));
    await refreshCodexAccounts();
    return;
  }
  codexLoginMoved();
}

/* 이 기계의 codex 로그인이 아예 없는가.
 *
 * 문장과 심각도가 **같은 판정**을 읽어야 한다. 원본도 그 행에 파괴색을 입히고
 * (`AccountsPane.tsx:1306`) 부제까지 파괴색으로 돌린다(`:1342`) — 관리 계정 행과
 * 같은 대우다. 조건을 두 곳에 쓰면 언젠가 한쪽만 고쳐진다. */
function codexSystemSignedOut() {
  return !codexAccountReport.system_label && codexAccountReport.system_kind !== "api-key";
}

function codexSystemUnder() {
  if (codexSystemSignedOut()) {
    return t("settings.codexAccounts.systemNone", "~/.codex에 로그인이 없습니다");
  }
  if (codexAccountReport.system_label) return codexAccountReport.system_label;
  return t("settings.codexAccounts.systemKey", "API 키 로그인 — 이름이 없습니다");
}

function codexAccountRow(row) {
  const line = document.createElement("div");
  line.className = "agent-row account-row";
  const chosen = row.id === codexAccountReport.active;
  const pick = document.createElement("button");
  pick.className = "account-pick";
  pick.type = "button";
  pick.setAttribute("aria-pressed", chosen ? "true" : "false");
  if (chosen) pick.classList.add("is-active");
  pick.appendChild(accountMark(chosen));
  const body = document.createElement("span");
  body.className = "agent-row-body";
  const top = document.createElement("span");
  top.className = "account-topline";
  const who = document.createElement("span");
  who.className = "agent-row-name";
  who.textContent = row.label;
  top.appendChild(who);
  body.appendChild(top);
  const under = document.createElement("span");
  under.className = "agent-row-cmd";
  // 세 갈래, `accountUnder` 와 같은 결: 자격 증명이 아예 없다 / 있는데 죽었다 /
  // 살아 있다. 가운데가 이 조각이 낸 것이다 — 죽은 토큰이 워크스페이스 라벨을
  // 달고 정상 행으로 앉아 있었다.
  under.textContent = !row.signed_in
    ? t("settings.codexAccounts.signedOut", "로그인이 없습니다 — 지우고 다시 추가하세요")
    : row.login_expired
      ? t("settings.codexAccounts.expired", "로그인이 만료되었습니다 — 다시 로그인하세요")
      : (row.workspace_label ?? "");
  const alarm = paintAccountAlarm(line, accountAlarmed(row));
  if (alarm) top.appendChild(alarm);
  body.appendChild(under);
  pick.appendChild(body);
  pick.addEventListener("click", () => pickCodexAccount(row.id));
  line.appendChild(pick);
  // The same repair as the Claude half, and for the sharper version of the
  // same reason: this provider had NO road out of a dead token at all, so
  // deleting the account was the only one — and a Codex home carries the
  // `AGENTS.md` and the settings that were brought into it.
  const again = document.createElement("button");
  again.className = "account-relogin";
  again.type = "button";
  again.textContent = t("settings.accounts.relogin", "다시 로그인");
  again.addEventListener("click", () => reloginCodexAccount(row, again));
  line.appendChild(again);
  const drop = document.createElement("button");
  drop.className = "account-drop";
  drop.type = "button";
  drop.textContent = t("settings.accounts.remove", "지우기");
  drop.addEventListener("click", () => dropCodexAccount(row));
  line.appendChild(drop);
  return line;
}

/* Log in again into the account's own home — the repair that keeps the row,
 * the home and everything carried into it. The Claude pair is
 * `reloginClaudeAccount`; this one has no per-account usage cache to forget,
 * because the Codex half does not keep one keyed by account — the bar's figure
 * is simply read again. */
async function reloginCodexAccount(row, button) {
  if (addingCodexAccount) return;
  addingCodexAccount = true;
  button.disabled = true;
  paintCodexAccounts();
  try {
    codexAccountReport = await invoke("relogin_codex_account", { id: row.id });
  } catch (error) {
    showError(String(error));
    await refreshCodexAccounts();
    return;
  } finally {
    addingCodexAccount = false;
  }
  codexLoginMoved();
}

async function pickCodexAccount(id) {
  if ((id ?? null) === (codexAccountReport.active ?? null)) return;
  try {
    codexAccountReport = await invoke("select_codex_account", { id });
  } catch (error) {
    showError(String(error));
    await refreshCodexAccounts();
    return;
  }
  codexLoginMoved();
}

async function dropCodexAccount(row) {
  const said = await askConfirm({
    title: t("settings.accounts.removeAsk", "이 계정을 지울까요?"),
    body: t("settings.accounts.removeBody", "{{who}}의 로그인이 이 기계에서 지워집니다.", {
      who: row.label,
    }),
    confirm: t("settings.accounts.remove", "지우기"),
    deny: t("settings.accounts.keep", "그대로 두기"),
    danger: true,
  });
  if (said !== true) return;
  try {
    codexAccountReport = await invoke("remove_codex_account", { id: row.id });
  } catch (error) {
    showError(String(error));
    await refreshCodexAccounts();
    return;
  }
  // Removing the chosen account is a move back to the machine's own login.
  codexLoginMoved();
}

el("codex-account-add").addEventListener("click", async () => {
  if (addingCodexAccount) return;
  addingCodexAccount = true;
  paintCodexAccountAdd();
  try {
    codexAccountReport = await invoke("add_codex_account");
  } catch (error) {
    showError(String(error));
  } finally {
    addingCodexAccount = false;
  }
  codexLoginMoved();
});

/* ---- and the third: this machine's Google login ----
 *
 * The one card whose login this WINDOW makes. Claude and Codex each have a
 * CLI that owns its browser round trip, so those cards spawn it and wait;
 * Google has no such program here — zo runs `zo login google` — so the
 * backend drives the OAuth flow itself and writes the token where zo will
 * find it (`crates/zerocode-shell/src/google_login.rs`). One machine, one
 * Google login: there is nothing to pick, so the list is a single row and the
 * row's buttons are the whole surface.
 *
 * The consent screen opens in the window's OWN browser tab — the requester is
 * the window, so it lands in the current group. That tab cannot do a
 * Bluetooth passkey, which is why the sign-in state carries both a note about
 * 「다른 방법 시도」 and a button that hands the same URL to the system
 * browser. */
let googleAccount = { signed_in: false, store: "" };
/* Where the sign-in is waiting, held only while it waits. It is what the
 * secondary button hands to the system browser, and it is also this card's
 * one piece of state: non-null MEANS a sign-in is in flight. */
let googleConsentUrl = null;
/* Whether the wait that is ending was cancelled by the person. A cancel
 * leaves the login on disk exactly as it was, so the bar has nothing to
 * re-read — and `googleLoginMoved` exists to force a read. */
let googleLoginCancelled = false;

async function refreshGoogleAccount() {
  try {
    googleAccount = (await invoke("google_account")) ?? { signed_in: false, store: "" };
  } catch (error) {
    showError(error);
  }
  paintGoogleAccount();
}

function paintGoogleAccount() {
  const host = el("google-account-list");
  host.replaceChildren();
  host.appendChild(googleLoginRow());
  say(el("google-account-note"), () => googleAccountNote());
}

/* This machine's Google login as a row.
 *
 * `ownLoginRow` is the shape the other two cards' system rows use and this is
 * deliberately NOT it: that row is a radio — chosen exactly when no managed
 * account is — and pressing it means "use this login". Here there is only one
 * login and nothing to choose, so a pressed-state control would be a radio
 * with one option. The classes are the same ones, so the three rows in this
 * pane read as three rows of one list. */
function googleLoginRow() {
  const line = document.createElement("div");
  line.className = "agent-row account-row";
  const body = document.createElement("span");
  body.className = "agent-row-body account-body";
  const top = document.createElement("span");
  top.className = "account-topline";
  const who = document.createElement("span");
  who.className = "agent-row-name";
  who.textContent = googleAccountName();
  top.appendChild(who);
  // 심각도는 두 카드와 같은 한 손에서 온다 — 행의 표시와 배지, 부제의 색까지.
  const alarm = paintAccountAlarm(line, googleLoginAlarmed());
  if (alarm) top.appendChild(alarm);
  body.appendChild(top);
  const under = document.createElement("span");
  under.className = "agent-row-cmd";
  under.textContent = googleLoginUnder();
  body.appendChild(under);
  line.appendChild(body);
  for (const action of googleLoginActions()) {
    const button = document.createElement("button");
    button.className = action.className;
    button.type = "button";
    button.textContent = action.label;
    button.addEventListener("click", () => action.press(button));
    line.appendChild(button);
  }
  return line;
}

function googleAccountName() {
  if (googleConsentUrl !== null) {
    return t("settings.googleAccount.signingIn", "브라우저에서 로그인 중…");
  }
  if (!googleAccount.signed_in) {
    return t("settings.googleAccount.signedOut", "로그인 안 됨");
  }
  // A login zo made, or one this window made before it cached the name: the
  // store holds tokens and no identity, and asking Google on every paint is
  // not what a name on a card is worth.
  return googleAccount.email ?? t("settings.googleAccount.unnamed", "이름을 확인하지 못했습니다");
}

/* 이 로그인이 지금 쓸 수 없거나 부족한가.
 *
 * 두 갈래를 한 판정으로 접는다: 갱신할 수 없는 로그인(refresh token 이 없다)과
 * 에이전트 스코프가 없는 로그인. 둘 다 「다시 로그인」이 고치는 것이고, 무엇이
 * 문제인지는 부제가 말한다 — 두 카드의 `accountAlarmed` 와 같은 결이다. */
function googleLoginAlarmed() {
  if (!googleAccount.signed_in || googleConsentUrl !== null) return false;
  return !googleAccount.renewable || !googleAccount.scoped_for_agents;
}

function googleLoginUnder() {
  if (googleConsentUrl !== null) {
    return t(
      "settings.googleAccount.waiting",
      "Google 동의 화면에서 계정을 고르면 이 창으로 돌아옵니다.",
    );
  }
  if (googleAccount.error) return googleAccount.error;
  if (!googleAccount.signed_in) {
    return t("settings.googleAccount.none", "zo와 Antigravity 게이지가 쓸 로그인이 없습니다");
  }
  if (!googleAccount.renewable) {
    return t(
      "settings.googleAccount.notRenewable",
      "토큰을 갱신할 수 없는 로그인입니다 — 다시 로그인하세요",
    );
  }
  if (!googleAccount.scoped_for_agents) {
    return t(
      "settings.googleAccount.narrowScope",
      "에이전트 권한 없이 만들어진 로그인입니다 — 다시 로그인하세요",
    );
  }
  return googleAccount.store;
}

function googleLoginActions() {
  if (googleConsentUrl !== null) {
    return [
      {
        className: "account-relogin",
        label: t("settings.googleAccount.openInSystem", "시스템 브라우저에서 열기"),
        press: () => openGoogleConsentInSystemBrowser(),
      },
      {
        className: "account-drop",
        label: t("settings.googleAccount.cancel", "취소"),
        press: () => cancelGoogleLogin(),
      },
    ];
  }
  if (!googleAccount.signed_in) {
    return [
      {
        className: "account-relogin",
        label: t("settings.googleAccount.signIn", "로그인"),
        press: (button) => startGoogleLogin(button),
      },
    ];
  }
  return [
    {
      className: "account-relogin",
      label: t("settings.accounts.relogin", "다시 로그인"),
      press: (button) => startGoogleLogin(button),
    },
    {
      className: "account-logout",
      label: t("settings.accounts.logout", "로그아웃"),
      press: () => googleLogout(),
    },
  ];
}

function googleAccountNote() {
  if (googleConsentUrl !== null) {
    // Google OAuth runs in the external user-agent; this row explains why the
    // settings card stays in a waiting state after that browser opens.
    return t(
      "settings.googleAccount.systemBrowserHint",
      "Google 로그인은 시스템 브라우저에서 진행됩니다. 계정을 고르면 이 창으로 자동으로 돌아옵니다.",
    );
  }
  return t(
    "settings.googleAccount.hint",
    "zo와 이 창이 같은 파일의 같은 로그인을 씁니다 — 여기서 로그인하면 zo도, Antigravity 사용량 게이지도 그 로그인을 씁니다.",
  );
}

/* Sign in, or sign in again — one road, because Google's own consent screen
 * is where the choice of account is made and 「다시 로그인」 is that same
 * screen. `prompt=consent` on the way out is what makes a repeat sign-in
 * still hand over a refresh token. */
async function startGoogleLogin(button) {
  if (googleConsentUrl !== null) return;
  button.disabled = true;
  let url;
  try {
    url = await invoke("google_login_start");
  } catch (error) {
    showError(String(error));
    paintGoogleAccount();
    return;
  }
  googleConsentUrl = url;
  googleLoginCancelled = false;
  paintGoogleAccount();
  // Google desktop OAuth must use an external user-agent. The loopback
  // listener above owns completion, so the system browser can hand control
  // back without coupling the flow to either browser surface.
  void openGoogleConsentInSystemBrowser();
  let report;
  try {
    report = await invoke("google_login_finish");
  } catch (error) {
    googleConsentUrl = null;
    showError(String(error));
    await refreshGoogleAccount();
    return;
  }
  googleConsentUrl = null;
  googleAccount = report;
  // A cancelled wait wrote nothing, so there is nothing for the bar to
  // re-read — forcing a scan there would spend a round trip to learn what it
  // already knows.
  if (googleLoginCancelled) paintGoogleAccount();
  else googleLoginMoved();
}

/* Open the same pending consent URL in the browser chosen by the operating
 * system. Repeating this is harmless and lets a person recover after closing
 * the browser before the loopback redirect completes. */
async function openGoogleConsentInSystemBrowser() {
  if (googleConsentUrl === null) return;
  try {
    await invoke("open_url", { url: googleConsentUrl });
  } catch (error) {
    showError(String(error));
  }
}

async function cancelGoogleLogin() {
  googleLoginCancelled = true;
  try {
    await invoke("cancel_google_login");
  } catch (error) {
    showError(String(error));
  }
  // The waiting `google_login_finish` answers a moment later with what it
  // found and repaints this row; there is nothing to paint here.
}

/* Take this login off the machine. Asked first, the way the other two cards
 * ask: this is the credential zo runs on as well, and the file it lives in
 * holds other products' logins that must not go with it. */
async function googleLogout() {
  const said = await askConfirm({
    title: t("settings.googleAccount.logoutAsk", "Google 로그인을 로그아웃할까요?"),
    body: t(
      "settings.googleAccount.logoutBody",
      "{{path}}에서 Google 로그인만 지워집니다. zo의 다른 자격 증명은 그대로입니다.",
      { path: googleAccount.store },
    ),
    confirm: t("settings.accounts.logout", "로그아웃"),
    deny: t("settings.accounts.keep", "그대로 두기"),
    danger: true,
  });
  if (said !== true) return;
  try {
    googleAccount = await invoke("google_logout");
  } catch (error) {
    showError(String(error));
    await refreshGoogleAccount();
    return;
  }
  googleLoginMoved();
}

/* The Claude and Codex pair are `claudeLoginMoved` and `codexLoginMoved`, and
 * this is the same hand for the same reason: the bar's Antigravity figure is
 * a fact about THIS login, and without a forced re-read the segment keeps the
 * previous one's word — 「Antigravity 로그인 필요」 standing under an account
 * that just signed in — until an agent stirs or the fifteen-minute poll comes
 * round. The verbs above all wait for the backend to finish writing before
 * they answer, so by the time this runs the new login is on disk. */
function googleLoginMoved() {
  paintGoogleAccount();
  void refreshProviderUsage(usageProvider("antigravity"), true);
}

/* ---- and the fourth: every CLI that signs itself in ----
 *
 * Grok, Kimi, and whichever provider gains an OAuth login next. Claude and
 * Codex each grew a card above by hand; these rows come off ONE table the
 * backend owns (`crates/zerocode-shell/src/cli_login.rs`): a row says which
 * road its login takes and this side walks it, never asking which agent it
 * is looking at. `verb` — the CLI has a headless login verb; the backend
 * runs it in the row's home and waits for the credential file, the way the
 * Codex card does. `tui` — the CLI only signs in from inside its own screen
 * (Kimi's `/login`), so the window opens the CLI in one of its panes, types
 * the row's command at it, and the backend watches the file. `first-run` —
 * the bare program signs in the first time it runs; the pane is opened and
 * the file watched, nothing typed. The file is read and never written, here
 * or in the backend: Kimi rotates its refresh token, and a copy written by
 * anybody but the CLI logs the live session out. */
let cliLogins = { rows: [] };
/* Rows with a login or logout in flight, by agent id, carrying the road and
 * the command being waited on — the caption says so and the buttons refuse
 * a second press: two browser logins racing for one credential file is the
 * mistake this guard exists to prevent. */
const cliLoginBusy = new Map();
/* How many reads of the table this window has asked for. Two can be out at
 * once — the pane's arrival and a walk's end — and the one that answers last
 * is not always the one asked last: a row proven by a status command holds
 * its read for seconds. Only the newest read paints, so a late answer never
 * paints over a table read after it (t-7170 R2b). */
let cliLoginReads = 0;

async function refreshCliLogins() {
  const read = ++cliLoginReads;
  let table;
  try {
    table = (await invoke("cli_login_list")) ?? { rows: [] };
  } catch (error) {
    if (read !== cliLoginReads) return;
    showError(error);
    table = { rows: [] };
  }
  if (read !== cliLoginReads) return;
  cliLogins = table;
  // The status bar's sign-in buttons read the same table: a provider with a
  // row here has somewhere to send a signed-out gauge.
  noteCliLoginRows(cliLogins.rows ?? []);
  paintCliLogins();
}

function paintCliLogins() {
  const host = el("cli-login-list");
  host.replaceChildren();
  const rows = cliLogins.rows ?? [];
  for (const row of rows) host.appendChild(cliLoginRow(row));
  say(el("cli-login-note"), () =>
    rows.length === 0
      ? t("settings.cliLogins.noRows", "이 창이 아는 CLI 로그인 길이 없습니다")
      : t(
          "settings.cliLogins.hint",
          "각 CLI가 제 홈에 남긴 자격 증명 파일을 창이 읽기만 합니다 — 로그인과 로그아웃은 그 CLI가 합니다.",
        ),
  );
}

/* One provider as a row: the CLI's name, who is signed in (or why nobody
 * is), and the verbs this machine can take. The classes the three cards
 * above wear, so the pane reads as one list. Not `ownLoginRow`: that row is
 * a radio — chosen exactly when no managed account is — and there is
 * nothing to choose here. */
function cliLoginRow(row) {
  const line = document.createElement("div");
  line.className = "agent-row account-row";
  const body = document.createElement("span");
  body.className = "agent-row-body account-body";
  const top = document.createElement("span");
  top.className = "account-topline";
  const who = document.createElement("span");
  who.className = "agent-row-name";
  who.textContent = row.name;
  top.appendChild(who);
  body.appendChild(top);
  const under = document.createElement("span");
  under.className = "agent-row-cmd";
  under.textContent = cliLoginUnder(row);
  body.appendChild(under);
  line.appendChild(body);
  for (const action of cliLoginActions(row)) {
    const button = document.createElement("button");
    button.className = action.className;
    button.type = "button";
    button.textContent = action.label;
    button.disabled = Boolean(action.disabled);
    button.addEventListener("click", () => action.press(button));
    line.appendChild(button);
  }
  return line;
}

function cliLoginUnder(row) {
  const busy = cliLoginBusy.get(row.agent);
  if (busy?.road === "verb") {
    return t("settings.cliLogins.signingIn", "브라우저에서 로그인 중…");
  }
  if (busy?.road === "run-once") {
    return t(
      "settings.cliLogins.waitingRefresh",
      "터미널 판에서 {{program}}이(가) 세션을 갱신하면 이 행이 갱신됩니다",
      { program: row.program },
    );
  }
  if (busy?.command) {
    return t(
      "settings.cliLogins.waitingTui",
      "터미널 판에서 {{command}}을(를) 마치면 이 행이 갱신됩니다",
      { command: busy.command },
    );
  }
  if (busy) {
    return t("settings.cliLogins.waitingPane", "터미널 판에서 로그인을 마치면 이 행이 갱신됩니다");
  }
  if (!row.installed) {
    return t(
      "settings.cliLogins.notInstalled",
      "{{program}}을(를) PATH에서 찾지 못했습니다 — 설치 안내는 오른쪽 링크",
      { program: row.program },
    );
  }
  // Two rows the window must not guess about. `none` is a CLI that keeps
  // its login in a keyring with no status command behind it: the road still
  // works, and the CLI's own screen is where the answer is. The other is a
  // status command that did not answer — which is not 「로그아웃됨」 either.
  if (!row.known) {
    return row.proof === "none"
      ? t(
          "settings.cliLogins.unreadable",
          "{{program}}은(는) 로그인을 창이 읽을 수 없는 곳에 둡니다 — 로그인은 여기서 열고, 결과는 그 CLI에서 확인하세요",
          { program: row.program },
        )
      : t("settings.cliLogins.unanswered", "{{program}}이(가) 로그인 상태에 답하지 않았습니다", {
          program: row.program,
        });
  }
  if (!row.signed_in) {
    return t("settings.cliLogins.signedOut", "{{home}}에 로그인이 없습니다", { home: row.home });
  }
  // A session on file whose renewal is the CLI's own next run (Grok, t-7170):
  // signed in as far as the file goes, and still not a login the gauge can
  // read with — the caption says which program to run, not whose it is.
  if (cliLoginExpired(row)) {
    return t(
      "settings.cliLogins.expired",
      "로그인 만료 — {{program}}을(를) 한 번 실행하면 CLI가 세션을 갱신합니다",
      { program: row.program },
    );
  }
  return row.account ?? row.home;
}

/* Whether this row's gauge says the session on file has expired and its
 * renewal belongs to the CLI's next run. Read off the gauge rather than the
 * file: the file holds a session either way, and only the read knows the
 * session no longer answers. */
function cliLoginExpired(row) {
  const provider = USAGE_PROVIDERS.find((one) => one.id === row.agent);
  return provider ? usageNeedsRun(provider.read().usage) : false;
}

/* Whether the window can open this CLI bare: through the launch door when
 * the catalog knows it, or by the table's own shell line when it does not.
 * A row with neither has no 「한 번 실행」 — a button that could only fail
 * is worse than none. */
function cliLoginRunsBare(row) {
  return row.opens === "agent" || Boolean(row.run_command);
}

/* The verbs a row offers: the vendor's install page while the CLI is
 * absent (the same door the agent list's rows open), a sign-in while
 * nobody is signed in, and 다시 로그인 + 로그아웃 once somebody is. */
function cliLoginActions(row) {
  if (!row.installed) {
    if (!row.homepage_url) return [];
    return [
      {
        className: "account-relogin",
        label: t("settings.agents.install", "설치"),
        press: () => openExternal(row.homepage_url),
      },
    ];
  }
  const busy = cliLoginBusy.has(row.agent);
  const actions = [];
  // An expired session's first hand is the program itself, once: the CLI
  // renews its own session on its next run, and the window neither logs in
  // nor refreshes for it (the design `usage_grok.rs` records). The login
  // verb stays beside it for the person whose session is gone for good.
  if (cliLoginExpired(row) && cliLoginRunsBare(row)) {
    actions.push({
      className: "account-relogin",
      label: t("usage.runOnce", "{{program}} 한 번 실행", { program: row.program }),
      disabled: busy,
      press: (button) => runCliOnce(row, button),
    });
  }
  actions.push({
    className: "account-relogin",
    label: row.signed_in
      ? t("settings.accounts.relogin", "다시 로그인")
      : t("usage.signIn", "로그인"),
    disabled: busy,
    press: (button) => startCliLogin(row, button),
  });
  // The way out stands only where there IS one: several of these CLIs
  // document no logout at all, and a button that removed the file itself
  // would take a credential the CLI never said it could lose. A row whose
  // login this window cannot read has nothing to sign out of either — it
  // does not know whether anybody is signed in.
  if (row.signed_in && row.known && row.logout_road !== "none") {
    actions.push({
      className: "account-logout",
      label: t("settings.accounts.logout", "로그아웃"),
      disabled: busy,
      press: () => logoutCliLogin(row),
    });
  }
  return actions;
}

/* One walk for every road IN: mark the row busy with the road and the
 * command the caption names, walk it, and on the far side re-read the
 * table, the gauge and the agent list. The walk itself writes no row — a
 * verb's or a wait's answer is the table as it stood when THAT road ended,
 * and a road that ends late must not paint over rows the table has since
 * moved on from (astra R2b) — so the rows are re-read once here, on every
 * road, landed or not; a login the person abandoned leaves them exactly as
 * they were. The gauge is asked past its floor on the failed road too
 * (t-7170 R2): the CLI may have renewed the session in a way the watch
 * does not count as a change, and the gauge is the one reader that knows. */
async function walkCliLoginRoad(row, button, { road, command, walk }) {
  if (cliLoginBusy.has(row.agent)) return;
  button.disabled = true;
  cliLoginBusy.set(row.agent, { road, command });
  paintCliLogins();
  try {
    await walk();
  } catch (error) {
    showError(String(error));
  }
  cliLoginBusy.delete(row.agent);
  await refreshCliLogins();
  cliLoginMoved(row);
}

/* The credential file as it stands BEFORE the CLI runs (t-7170 R2), kept by
 * the backend under a number this window carries into the wait: a CLI that
 * renews on start, faster than the window's next call, is a change against
 * that snapshot and invisible to one taken when the wait begins. Nothing to
 * take for a row whose login this window cannot read. */
async function takeCliLoginWitness(row) {
  if (row.proof === "none") return null;
  return (await invoke("cli_login_witness", { agent: row.agent })) ?? null;
}

/* And then the backend watches the credential file against that witness —
 * unless the row has nothing to watch, where waiting would be waiting for
 * an answer that never comes. The answer is the walk's end, not a row: the
 * walk re-reads the table once it is over (`walkCliLoginRoad`). A witness
 * the backend will not honour — let go, ended by a newer attempt, past its
 * ceiling — is the wait's own refusal, and this road ends there. */
async function waitForCliLogin(row, witness) {
  if (row.proof === "none") return;
  await invoke("cli_login_wait", { agent: row.agent, signedIn: true, witness });
}

/* A witness nobody will wait on — the run it was taken for would not start
 * — is let go, so the backend holds nothing for a wait that is not coming. */
async function dropCliLoginWitness(witness) {
  if (witness === null) return;
  await invoke("cli_login_witness_drop", { witness });
}

/* The pane roads' order (t-7170 R2): the witness, then the run, then the
 * wait against that witness. A witness the backend would not take means no
 * run at all — a run nobody is watching is the hole this order closes —
 * and a run that would not start lets the witness go before the error
 * travels on. */
async function runThenWait(row, run) {
  const witness = await takeCliLoginWitness(row);
  try {
    await run();
  } catch (error) {
    await dropCliLoginWitness(witness);
    throw error;
  }
  await waitForCliLogin(row, witness);
}

/* Sign in, or sign in again — the row's road decides how. */
function startCliLogin(row, button) {
  return walkCliLoginRoad(row, button, {
    road: row.road,
    command: row.tui_login ?? row.pane_command ?? null,
    walk: async () => {
      if (row.road === "verb") {
        await invoke("cli_login_start", { agent: row.agent });
        return;
      }
      // `pane-verb` types the row's shell line into a plain shell; `tui`
      // types the row's slash command at its CLI; `first-run` opens it bare
      // — each between the witness and the wait.
      await runThenWait(row, async () => {
        if (row.road === "pane-verb") await typeCliLoginShellLine(row, row.pane_command);
        else await typeCliLoginCommand(row, row.tui_login ?? null);
      });
    },
  });
}

/* Run the CLI once, bare, so it renews the session it holds (t-7170): the
 * witness first, then the run — in a pane this window opens for it — then
 * the same watch on the file; a renewed session is a changed file, which is
 * what the watch calls arrival, and then the gauge is re-read past its
 * floor. */
function runCliOnce(row, button) {
  return walkCliLoginRoad(row, button, {
    road: "run-once",
    command: null,
    walk: () => runThenWait(row, () => runCliBare(row)),
  });
}

/* A run is a process START in a pane this window opens for it, and nowhere
 * else (astra m-7239 R1, R1b). Every pane the agent already holds is the
 * person's: one the program is still in would take the words as a prompt
 * to their session — the renewal is the CLI's next START, which a program
 * at its composer will not do — and one the program has left is a shell
 * whose line and foreground nothing here can vouch for at the instant of a
 * write (the pane table's `idle` is a word about the past, not about this
 * instant or about half a command typed since). So the run never goes to a
 * pane it did not just open: the launch door starts the CLI bare in a new
 * pane — the door spawns the program itself, so the pane IS the run — and
 * a CLI the launch catalog does not know has the table's bare line typed
 * into a new plain shell. */
async function runCliBare(row) {
  if (row.opens === "shell") return typeCliLoginShellLine(row, row.run_command);
  return openCliPane(row);
}

/* Open the CLI in a pane of this window and type `command` at it — or open
 * it bare when there is nothing to type. A pane the agent already holds and
 * is still in is reused and brought to the front: the command is a word to
 * a running program, not a reason to start a second one — and a pane the
 * program has left is not that pane. */
async function typeCliLoginCommand(row, command) {
  // A CLI the launch catalog does not know has no agent pane to open: the
  // window types its line into a plain shell instead, and the caption names
  // the command for the person to type at it.
  if (row.opens === "shell") return typeCliLoginShellLine(row, row.pane_command);
  let term = runningCliPaneOf(row);
  if (term === null) term = await openCliPane(row);
  else setActiveTab(tabOfTerm(term).id);
  if (command) {
    await invoke("send_prompt", { term, text: command, submit: true, agent: row.agent });
  }
  return term;
}

/* The one launch door every agent takes (`launchAgentTab`), with an EMPTY
 * prompt: a launch prompt has the orchestration contract appended to it,
 * which would turn a slash command into a paragraph. The words go through
 * `send_prompt` instead, whose readiness wait lands them in the composer
 * rather than in the CLI's boot banner. */
async function openCliPane(row) {
  const term = await launchAgentTab({ agent: row.agent, prompt: "", ...spawnGrid() });
  mountTermTab(term, { agent: row.name }, {});
  return term;
}

/* The pane this window holds for the row's agent that the program is still
 * in — the one a slash command is a word to. A pane the program has left is
 * a shell at its prompt, which the backend reports as `idle` once the
 * foreground group is the shell's own again (`paneProgramLeft`), and a
 * slash command is no word to a shell; a pane no tab holds was detached,
 * and a word typed at it lands where nobody looks. */
function runningCliPaneOf(row) {
  return (
    [...paneAgents]
      .filter(([term, agent]) => agent === row.agent && tabOfTerm(term) !== null)
      .map(([term]) => term)
      .find((term) => !paneProgramLeft(term)) ?? null
  );
}

/* A verb that needs a screen — a device code on stderr, or a CLI that
 * refuses a pipe — typed into a NEW plain shell of this window, the way the
 * GitLab card types `glab auth login` (`openGitlabLoginTerminal`): a shell
 * this window just opened is the one line it knows to be empty and its
 * own. The line is the backend's: the home in the CLI's own variable, then
 * the verb, so the CLI writes where the gauge reads. The settings panel
 * steps aside so the person can see the code they are being asked to
 * enter. */
async function typeCliLoginShellLine(row, line) {
  if (!line) throw new Error(t("settings.cliLogins.noLine", "이 행에는 입력할 명령이 없습니다"));
  setSettingsOpen(false);
  const term = await invoke("open_term_tab", { rows: 24, cols: 96, plain: true });
  mountTermTab(term);
  await invoke("term_text", { term, text: `${line}\r` });
  return term;
}

/* Sign out. Asked first, the way the other cards ask: the credential this
 * removes is the one every terminal running that CLI signs in with. */
async function logoutCliLogin(row) {
  if (cliLoginBusy.has(row.agent)) return;
  const said = await askConfirm({
    title: t("settings.cliLogins.logoutAsk", "{{name}} 로그인을 로그아웃할까요?", { name: row.name }),
    body: t(
      "settings.cliLogins.logoutBody",
      "{{home}}의 자격 증명이 지워집니다. 다른 CLI의 로그인은 그대로입니다.",
      { home: row.home },
    ),
    confirm: t("settings.accounts.logout", "로그아웃"),
    deny: t("settings.accounts.keep", "그대로 두기"),
    danger: true,
  });
  if (said !== true) return;
  cliLoginBusy.set(row.agent, {
    road: row.logout_road,
    command: row.tui_logout ?? row.logout_command ?? null,
  });
  paintCliLogins();
  // The answer is the road's end, not a row — the table is re-read once the
  // road is over, as every road in does (`walkCliLoginRoad`).
  try {
    if (row.logout_road === "verb") {
      await invoke("cli_login_logout", { agent: row.agent });
    } else {
      // The way out has the same three shapes the way in has: a verb that
      // needs a screen (it asks which provider, or asks twice), and a slash
      // command at the CLI's own screen.
      if (row.logout_road === "pane-verb") await typeCliLoginShellLine(row, row.logout_command);
      else await typeCliLoginCommand(row, row.tui_logout ?? null);
      await invoke("cli_login_wait", { agent: row.agent, signedIn: false });
    }
  } catch (error) {
    showError(String(error));
    cliLoginBusy.delete(row.agent);
    await refreshCliLogins();
    return;
  }
  cliLoginBusy.delete(row.agent);
  await refreshCliLogins();
  cliLoginMoved(row);
}

/* The Claude, Codex and Google cards each have this hand, for the same
 * reason: the bar's figure for this provider is a fact about ONE login, and
 * without a forced re-read the segment keeps the previous login's word until
 * the fifteen-minute poll comes round. The rows are already fresh — every
 * road re-reads the table once it is over — so only the gauge and the agent
 * list are asked again. */
function cliLoginMoved(row) {
  noteCliLoginRows(cliLogins.rows ?? []);
  paintCliLogins();
  const provider = USAGE_PROVIDERS.find((one) => one.id === row.agent);
  if (provider) void refreshProviderUsage(provider, true);
  void refreshAgents();
}

// Once at boot, so the status bar's sign-in buttons know their roads before
// anybody opens the accounts pane; the pane re-reads on every arrival.
void refreshCliLogins();

/* ---- where a card stands, in four words ----
 *
 * Every card on this screen that asks something of a server can be in one of
 * four standings, and until now each card invented its own vocabulary for
 * them: the TypeSafe card was the only one wearing a badge at all, and the
 * two words it wore (키 저장됨 / 키 없음) were about a keychain rather than
 * about a connection, so the router card beside it said nothing and a person
 * had to read a status line to find out whether it worked.
 *
 * One table, four rows, each naming the badge face it wears. Nothing else in
 * this file spells one of these words — `paintSettingsStanding` is the only
 * reader, and a card names a row's id. */
const SETTINGS_STANDINGS = [
  { id: "connected", state: "connected", key: "settings.standing.connected", word: "연결됨" },
  { id: "keySaved", state: "available", key: "settings.standing.keySaved", word: "키 저장됨" },
  { id: "unchecked", state: "unchecked", key: "settings.standing.unchecked", word: "확인 안 됨" },
  { id: "failed", state: "error", key: "settings.standing.failed", word: "연결 실패" },
];

function settingsStanding(id) {
  return SETTINGS_STANDINGS.find((row) => row.id === id) ?? null;
}

/* A card's head says where it stands. */
function paintSettingsStanding(host, id) {
  const row = settingsStanding(id);
  if (!host || !row) return;
  host.dataset.state = row.state;
  host.textContent = t(row.key, row.word);
}

/* ---- one status line, for every card that asks a server something ----
 *
 * The standing as a badge, one sentence of ours, and — only when a server
 * actually said something — a fold holding what it said with the request id
 * it named.
 *
 * What this replaces: the server's own words as the first line a person read.
 * A refused key came back here as `서버 오류: HTTP 401 — 无效的令牌 (request
 * id: …)` — a 120-character line that wrapped to two, in a language neither
 * the window nor the reader had chosen, with a 23-digit id in the middle of
 * it. None of that says what to do next; all of it is what a support thread
 * needs. So it moves one fold down and our sentence takes the first line. */
function paintSettingsStatus(host, said, { standing = null, raw = null } = {}) {
  if (!host) return;
  host.replaceChildren();
  if (!said) return;
  if (standing) {
    const badge = document.createElement("span");
    badge.className = "settings-state";
    paintSettingsStanding(badge, standing);
    host.appendChild(badge);
  }
  const words = document.createElement("p");
  words.className = "settings-status-said";
  words.textContent = said;
  host.appendChild(words);
  const evidence = serverEvidence(raw);
  if (evidence) host.appendChild(evidence);
}

/* What the server said, word for word, under a fold that opens on demand —
 * with the request id pulled onto its own line, because that is the one token
 * in a refusal a person is ever asked to quote back. */
function serverEvidence(raw) {
  const said = String(raw ?? "").trim();
  if (!said) return null;
  const fold = document.createElement("details");
  fold.className = "settings-fold settings-status-raw";
  const summary = document.createElement("summary");
  summary.textContent = t("settings.status.evidence", "자세히");
  fold.appendChild(summary);
  const id = requestIdIn(said);
  if (id) {
    const line = document.createElement("p");
    line.className = "settings-status-id";
    line.textContent = t("settings.status.requestId", "요청 id — {{id}}", { id });
    fold.appendChild(line);
  }
  const body = document.createElement("pre");
  body.className = "settings-status-body";
  body.textContent = said;
  fold.appendChild(body);
  return fold;
}

/* `request id: 2026…`, `request_id=…`, `"x-request-id": "…"` — the one part of
 * a refusal body that reads the same whatever language the rest of it is in. */
function requestIdIn(said) {
  return said.match(/request[ _-]?id["'\s:=]+([A-Za-z0-9_-]{6,64})/i)?.[1] ?? null;
}

/* ---- API Routers (OpenRouter, AgentRouter, Custom) ----
 *
 * §1.0 contract:
 * - One providers table: ~/.zo/settings.json providers[]
 * - Presets as data rows
 * - No code branch on router names
 * - Model lists come only from live GET models.path
 * - Reuse accountRow component styling
 * - Reuse OpenAI-compatible GET /models fetch as one function
 */

let routerPresets = [];
let configuredRouters = [];
let testedModels = [];
let apiRoutersInitialized = false;
let routerProbeGeneration = 0;
let testedRouterGeneration = null;
let routerProbeInFlight = null;
let routerSaving = false;
let routerDraftGeneration = 0;

function routerDraftChanged() {
  routerDraftGeneration += 1;
  clearRouterStatus();
  updateRouterSaveButtonState();
}

/* An edited draft has not been tried, so the card goes back to saying so and
 * the line it said it on is taken away — one card, one standing. */
function clearRouterStatus() {
  paintSettingsStatus(el("router-test-status"), "");
  paintSettingsStanding(el("router-card-state"), "unchecked");
}

/* A model list belongs to the connection that answered it. Editing that
 * connection invalidates both the list and any answer still on its way. */
function invalidateRouterProbe() {
  routerProbeGeneration += 1;
  routerDraftGeneration += 1;
  testedRouterGeneration = null;
  routerProbeInFlight = null;
  testedModels = [];
  el("router-models-tbody")?.replaceChildren();
  const section = el("router-models-section");
  if (section) section.hidden = true;
  clearRouterStatus();
  const test = el("router-test-btn");
  if (test) {
    test.disabled = false;
    test.textContent = t("settings.apiRouters.testConnection", "연결 시험");
  }
  updateRouterSaveButtonState();
}

/* One function for OpenAI-compatible GET /models fetch */
async function fetchRouterModels({ base_url, auth, key, models_path, id_field, context_field, client_fingerprint, headers }) {
  return await invoke("test_router_connection", {
    baseUrl: base_url,
    auth,
    key: key || null,
    modelsPath: models_path || "/models",
    idField: id_field || "id",
    contextField: context_field || "context_length",
    clientFingerprint: client_fingerprint || null,
    headers: headers || null,
  });
}

function initApiRoutersEvents() {
  if (apiRoutersInitialized) return;
  apiRoutersInitialized = true;

  const presetSelect = el("router-preset-select");
  if (presetSelect) {
    presetSelect.addEventListener("change", () => {
      onRouterPresetChange(presetSelect.value);
    });
  }

  const testBtn = el("router-test-btn");
  if (testBtn) {
    testBtn.addEventListener("click", () => {
      void runRouterTestConnection();
    });
  }

  const saveBtn = el("router-save-btn");
  if (saveBtn) {
    saveBtn.addEventListener("click", () => {
      void saveApiRouterEntry();
    });
  }

  const tbody = el("router-models-tbody");
  if (tbody) {
    tbody.addEventListener("change", () => {
      routerDraftChanged();
    });
  }
  for (const id of ["router-base-url-input", "router-key-input"]) {
    el(id)?.addEventListener("input", invalidateRouterProbe);
  }
  el("router-name-input")?.addEventListener("input", routerDraftChanged);
}

async function refreshApiRouters() {
  initApiRoutersEvents();
  try {
    if (routerPresets.length === 0) {
      routerPresets = (await invoke("api_router_presets")) ?? [];
      populateRouterPresets();
    }
  } catch (error) {
    showError(error);
  }

  try {
    paintRouterKeyStore(Boolean(await invoke("api_router_keys_kept")));
  } catch (error) {
    showError(error);
  }

  try {
    configuredRouters = (await invoke("api_router_providers")) ?? [];
  } catch (error) {
    showError(error);
  }
  paintConfiguredRouters();
  await refreshTypeSafe();
}

/* Where a router's key would go, said before anyone types one: a machine
 * with no keychain (every build but macOS) shows the other hint and offers no
 * key field, rather than promising a keychain the save then refuses. The
 * save's own refusal (`routerSaveRefusal`) stays as the backstop. */
function paintRouterKeyStore(kept) {
  const keptHint = el("router-keychain-hint");
  const noStoreHint = el("router-no-keychain-hint");
  if (keptHint) keptHint.hidden = !kept;
  if (noStoreHint) noStoreHint.hidden = kept;
  const keyInput = el("router-key-input");
  if (keyInput) {
    keyInput.disabled = !kept;
    if (!kept) keyInput.value = "";
  }
}

/* ---- TypeSafe (Jev) ----
 *
 * The one switch a person turns Jev on and off with (2026-09-23,
 * docs/design/jev-settings-20260917.md §6.1), and the key Jev is asked with.
 * The key goes into the keychain item every zo reads and never comes back to
 * this page: the backend only says whether one is saved
 * (`typesafe_settings`). Whether a saved key works is zo's answer, not this
 * page's — `check_typesafe_key` execs `zo decision-shadow check`, the shadow's
 * own question through the same key road. One action at a time: a save, a
 * removal, a check and a switch each repaint from the answer they get. Where
 * each feature stands is the dashboard's (shell-jev.js). */
let typesafeState = null;
let typesafeBusy = false;
let typesafeInitialized = false;
/* What the last CHECK answered, which is the only thing that can say
 * 연결됨 or 연결 실패 about this key — a saved key is a saved key until
 * somebody asks TypeSafe. `null` means nobody has asked since it was saved,
 * and the standing is read off the key itself. */
let typesafeChecked = null;

/* The switch's own paint and door live in shell-jev.js
 * (`paintJevSwitches`, `setJevEnabled`): one state and one command for this
 * card and for the dashboard, which wears the same switch over its table. */

function initTypeSafeEvents() {
  if (typesafeInitialized) return;
  typesafeInitialized = true;
  el("typesafe-key-input")?.addEventListener("input", paintTypeSafeSave);
  el("typesafe-save-btn")?.addEventListener("click", () => {
    void saveTypeSafeKey();
  });
  el("typesafe-remove-btn")?.addEventListener("click", () => {
    void runTypeSafe(() => {
      typesafeChecked = null;
      return invoke("remove_typesafe_key");
    }, () => t("settings.typesafe.removed", "저장된 키를 지웠습니다."));
  });
  el("typesafe-check-btn")?.addEventListener("click", () => {
    void checkTypeSafeKey();
  });
  el("jev-enabled")?.addEventListener("change", (event) => {
    void setJevEnabled(event.target.checked);
  });
  el("typesafe-card")?.querySelector("[data-jev-everywhere]")?.addEventListener("click", () => {
    void setJevEnabled(true);
  });
  el("jev-open-dashboard")?.addEventListener("click", () => {
    setSettingsOpen(false);
    openJevView();
  });
  el("route-classifier-select")?.addEventListener("change", (event) => {
    const mode = event.target.value;
    void runTypeSafe(() => invoke("set_route_classifier", { mode }), (state) => {
      if (!state.classifier?.reaches) {
        return t("settings.classifier.nowOff", "자동 라우팅을 껐습니다 — 「모델 선택 판단」도 판단을 요청하지 않습니다.");
      }
      return state.classifier.probes
        ? t("settings.classifier.nowProbes", "이제 빠른 등급 모델에게도 묻습니다.")
        : t("settings.classifier.nowQuiet", "바꿨습니다 — 이 방식은 빠른 등급 모델에게 묻지 않습니다.");
    });
  });
  // The model pin (`smart.jevModel`): an empty field unpins. The backend
  // refuses a pin its door would not read, and the refusal is said here.
  el("typesafe-model-input")?.addEventListener("change", (event) => {
    const model = event.target.value;
    void runTypeSafe(() => invoke("set_jev_model", { model }), (state) =>
      state.model?.pinned
        ? t("settings.typesafe.modelPinned", "{{model}} 버전으로 고정했습니다. 다음 요청부터 이 버전을 씁니다.",
          { model: state.model.model })
        : t("settings.typesafe.modelUnpinned", "고정을 풀었습니다. 늘 최신 버전을 씁니다."));
  });
}

/* The model field as the backend answered it: the pin when there is one,
 * empty with the alias behind it when there is none. A field somebody is
 * typing in is left alone. */
function paintTypeSafeModel(state) {
  const input = el("typesafe-model-input");
  if (!input || !state.model) return;
  input.placeholder = state.model.alias;
  if (document.activeElement !== input) input.value = state.model.pinned ? state.model.model : "";
  input.disabled = typesafeBusy;
}

/* One seat's row of the backend's answer, by the use's own name. The words a
 * mode is spelled with are the Jev use table's; this page reads only what a
 * mode does — asks, applies, automatic — and spells none of them. */
function jevSeat(state, seat) {
  return state.switches?.find((row) => row.id === seat) ?? null;
}

/* What a seat now stands at, as its own row describes it. */
function jevSeatChoice(state, seat) {
  const row = jevSeat(state, seat);
  return row?.modes?.find((choice) => choice.mode === row.mode) ?? null;
}

/* The routing classifier's four choices, each named by what it DOES — the same
 * rule the seat switches keep, and for the same reason: the words a classifier
 * setting may hold live in `zerocode_core::jev::ClassifierMode` and are
 * spelled nowhere else. What this page reads off a choice is whether the
 * classifier runs at all, whether it reads markers a person wrote, and whether
 * it calls a probe — and the last of those is also whether the routing seat
 * below is ever asked anything. */
function paintClassifierModes(select, choices) {
  select.replaceChildren();
  for (const choice of choices) {
    const option = document.createElement("option");
    option.value = choice.mode;
    if (!choice.runs) {
      option.textContent = t("settings.classifier.modeOff", "끔 — 자동 라우팅을 쓰지 않음");
    } else if (choice.probes) {
      option.textContent = t("settings.classifier.modeProbed", "낱말 + 모델에게도 물음");
    } else if (choice.markers) {
      option.textContent = t("settings.classifier.modeMarkers", "낱말 + 과업에 적힌 표식");
    } else {
      option.textContent = t("settings.classifier.modeWords", "낱말만");
    }
    select.appendChild(option);
  }
}

/* The feature the classifier gates, as the backend names it, and whether the
 * mode it stands at can reach anything at all. A feature that asks while
 * nothing calls a probe is a promise of a judgment that is never made — said
 * beside the classifier, which is what changes it. */
function paintClassifierGate(state) {
  const classifier = state.classifier;
  const select = el("route-classifier-select");
  if (select && classifier) {
    paintClassifierModes(select, classifier.modes ?? []);
    select.value = classifier.mode;
    select.disabled = typesafeBusy;
  }
  const notice = el("route-classifier-unreachable");
  if (notice) {
    // The feature is reached under every word but `off` (t-6346); the
    // probe is a separate question the warning does not ask.
    const asks = Boolean(classifier) && (jevSeatChoice(state, classifier.gates)?.asks ?? false);
    notice.hidden = !(asks && !classifier.reaches);
  }
}

async function refreshTypeSafe() {
  initTypeSafeEvents();
  try {
    paintTypeSafe(await invoke("typesafe_settings"));
  } catch (error) {
    paintTypeSafeStatus(typesafeRefusal(error), typesafeRefusalEvidence(error));
  }
  // The card draws no numbers; a dashboard on stage refreshes its own.
  if (jevViewsShowing()) void loadJevNumbers();
}

/* Where the card stands: what the last check answered if anything has been
 * asked since the key was saved, and otherwise whether there is a key at all.
 * A saved key is a saved key until somebody asks TypeSafe about it. */
function typesafeStanding(state) {
  return typesafeChecked ?? (state.keySaved ? "keySaved" : "unchecked");
}

function paintTypeSafe(state) {
  typesafeState = state;
  paintSettingsStanding(el("typesafe-key-state"), typesafeStanding(state));
  const input = el("typesafe-key-input");
  if (input) {
    input.disabled = typesafeBusy || !state.keysKeptHere;
    if (!state.keysKeptHere) input.value = "";
  }
  const noStore = el("typesafe-no-keychain-hint");
  if (noStore) noStore.hidden = state.keysKeptHere;
  for (const id of ["typesafe-check-btn", "typesafe-remove-btn"]) {
    const button = el(id);
    if (button) button.disabled = typesafeBusy || !state.keySaved;
  }
  paintTypeSafeModel(state);
  paintClassifierGate(state);
  paintTypeSafeSave();
  // The dashboard wears the same switch and reads the same features; a press
  // made here, or a fresh count, reaches it through this one paint.
  paintJevSwitches();
  paintJevViews();
}

function paintTypeSafeSave() {
  const save = el("typesafe-save-btn");
  if (save) {
    save.disabled = typesafeBusy || !typesafeState?.keysKeptHere ||
      !(el("typesafe-key-input")?.value.trim());
  }
}

function paintTypeSafeStatus(text, evidence = {}) {
  paintSettingsStatus(el("typesafe-status"), text, evidence);
}

/* One backend step, then the page the answer describes. A refused step keeps
 * the page as it was and says why. */
async function runTypeSafe(step, said) {
  if (typesafeBusy) return;
  typesafeBusy = true;
  if (typesafeState) paintTypeSafe(typesafeState);
  try {
    const state = await step();
    typesafeBusy = false;
    paintTypeSafe(state);
    // The line says where the card now stands as well as what just happened:
    // one component, and the standing is the same one its head wears.
    paintTypeSafeStatus(said(state), { standing: typesafeStanding(state) });
  } catch (error) {
    typesafeBusy = false;
    if (typesafeState) paintTypeSafe(typesafeState);
    paintTypeSafeStatus(typesafeRefusal(error), typesafeRefusalEvidence(error));
  }
}

async function saveTypeSafeKey() {
  const input = el("typesafe-key-input");
  const key = input?.value.trim() ?? "";
  if (!key) return;
  await runTypeSafe(async () => {
    // A different key is a different question: what the last check answered
    // was about the key that is being replaced.
    typesafeChecked = null;
    const state = await invoke("save_typesafe_key", { key });
    if (input) input.value = "";
    return state;
  }, () => t("settings.typesafe.saved", "키를 키체인에 저장했습니다. zo가 다음에 필요할 때 읽습니다."));
}

async function checkTypeSafeKey() {
  if (typesafeBusy) return;
  typesafeBusy = true;
  if (typesafeState) paintTypeSafe(typesafeState);
  paintTypeSafeStatus(t("settings.typesafe.checking", "TypeSafe에 묻는 중…"));
  try {
    const check = await invoke("check_typesafe_key");
    typesafeChecked = check.answered ? "connected" : "failed";
    paintTypeSafeStatus(
      check.answered
        ? t("settings.typesafe.answered", "응답했습니다 — {{model}}, {{ms}} ms", {
          model: check.model,
          ms: check.elapsedMs,
        })
        : typesafeCheckFailure(check.failure),
      { standing: typesafeChecked, raw: check.answered ? null : check.failure },
    );
  } catch (error) {
    typesafeChecked = "failed";
    paintTypeSafeStatus(
      t("settings.typesafe.unreachable", "확인하지 못했습니다 — zo가 답하지 않았습니다."),
      { standing: "failed", raw: String(error) },
    );
  } finally {
    typesafeBusy = false;
    if (typesafeState) paintTypeSafe(typesafeState);
  }
}

/* zo names why nothing answered with its closed failure table
 * (`api::SystemOneFailure::token`). The two a person acts on get words; any
 * other is a token, which belongs under the fold beside the rest of the
 * evidence rather than in the middle of our own sentence. */
function typesafeCheckFailure(token) {
  // The one table of failure tokens (`JEV_TOKENS`, shell-jev.js): the
  // dashboard's chips read the same rows (t-6243 D5).
  const row = jevTokenRow(token ?? "");
  return row?.saidKey ? t(row.saidKey, row.said) : t("settings.typesafe.unanswered", "응답하지 않았습니다.");
}

/* A refused step, in the reader's language when the backend named why — the
 * router pane's rule (`routerSaveRefusal`). */
function typesafeRefusal(error) {
  const failure = failureOf(error);
  return failure.kind === "keychain-unavailable"
    ? t(
      "settings.typesafe.keychainUnavailable",
      "이 컴퓨터에는 API 키를 보관할 키체인이 없어 저장하지 않았습니다.",
    )
    : failure.message;
}

/* A refused step says our sentence and keeps the backend's under the fold —
 * except when the refusal already IS our sentence, which is the one kind this
 * pane translates. */
function typesafeRefusalEvidence(error) {
  const failure = failureOf(error);
  return failure.kind === "keychain-unavailable" ? {} : { raw: failure.message };
}

function populateRouterPresets() {
  const select = el("router-preset-select");
  if (!select) return;
  select.replaceChildren();

  for (const preset of routerPresets) {
    const opt = document.createElement("option");
    opt.value = preset.id;
    opt.textContent = preset.name;
    select.appendChild(opt);
  }
  const customOpt = document.createElement("option");
  customOpt.value = "custom";
  customOpt.textContent = t("settings.apiRouters.customPreset", "직접 입력 (커스텀)");
  select.appendChild(customOpt);

  if (routerPresets.length > 0) {
    select.value = routerPresets[0].id;
    onRouterPresetChange(routerPresets[0].id);
  }
}

function onRouterPresetChange(chosenId) {
  const nameInput = el("router-name-input");
  const urlInput = el("router-base-url-input");
  const keyInput = el("router-key-input");
  invalidateRouterProbe();

  const found = routerPresets.find((p) => p.id === chosenId);
  if (found) {
    if (nameInput) nameInput.value = found.name;
    if (urlInput) urlInput.value = found.base_url;
    if (keyInput) keyInput.value = "";
  } else {
    // Custom
    if (nameInput) nameInput.value = "";
    if (urlInput) urlInput.value = "";
    if (keyInput) keyInput.value = "";
  }
}

function routerAccountRow(row) {
  const line = document.createElement("div");
  line.className = "agent-row account-row";
  const body = document.createElement("span");
  body.className = "agent-row-body account-body";
  const top = document.createElement("span");
  top.className = "account-topline";
  const who = document.createElement("span");
  who.className = "agent-row-name";
  who.textContent = row.name;
  top.appendChild(who);
  body.appendChild(top);
  const under = document.createElement("span");
  under.className = "agent-row-cmd";
  const count = row.models?.length ?? 0;
  under.textContent = t("settings.apiRouters.rowSummary", "{{base}} · 모델 {{count}}개", {
    base: row.base_url,
    count,
  });
  body.appendChild(under);
  line.appendChild(body);

  const drop = document.createElement("button");
  drop.className = "account-drop";
  drop.type = "button";
  drop.textContent = t("settings.accounts.remove", "지우기");
  drop.addEventListener("click", () => void dropConfiguredRouter(row));
  line.appendChild(drop);
  return line;
}

function paintConfiguredRouters() {
  const list = el("router-provider-list");
  if (!list) return;
  list.replaceChildren();

  const countEl = el("router-provider-count");
  if (countEl) {
    countEl.textContent = configuredRouters.length > 0 ? String(configuredRouters.length) : "";
  }

  for (const row of configuredRouters) {
    list.appendChild(routerAccountRow(row));
  }

  const noteEl = el("router-provider-note");
  if (noteEl) {
    noteEl.textContent = configuredRouters.length === 0
      ? t("settings.apiRouters.noConnected", "연결된 API 라우터가 없습니다.")
      : "";
  }
}

/* A row is removed by its name. Which key it read is the backend's to find, on
 * the row itself: a guess from the display name deleted some other item and
 * left the real key behind. */
async function dropConfiguredRouter(row) {
  try {
    configuredRouters = (await invoke("remove_api_router", { name: row.name })) ?? [];
  } catch (error) {
    showError(error);
  }
  paintConfiguredRouters();
}

async function runRouterTestConnection() {
  const testBtn = el("router-test-btn");
  const saveBtn = el("router-save-btn");
  const statusEl = el("router-test-status");
  const nameInput = el("router-name-input");
  const urlInput = el("router-base-url-input");
  const keyInput = el("router-key-input");
  const select = el("router-preset-select");
  const modelsSection = el("router-models-section");
  const tbody = el("router-models-tbody");

  const baseUrl = urlInput?.value.trim() ?? "";
  const key = keyInput?.value.trim() ?? "";
  if (!baseUrl) {
    paintSettingsStatus(
      statusEl,
      t("settings.apiRouters.needBaseUrl", "기본 URL이 비어 있습니다 — 라우터의 주소를 적으세요."),
      { standing: "failed" },
    );
    paintSettingsStanding(el("router-card-state"), "failed");
    return;
  }

  const chosenId = select?.value ?? "custom";
  const preset = routerPresets.find((p) => p.id === chosenId);

  const auth = preset?.auth ?? { header: "Authorization", scheme: "Bearer" };
  const modelsConfig = preset?.models ?? { path: "/models", id_field: "id", context_field: "context_length" };

  invalidateRouterProbe();
  const generation = routerProbeGeneration;
  routerProbeInFlight = generation;

  if (testBtn) {
    testBtn.disabled = true;
    testBtn.textContent = t("settings.apiRouters.testing", "연결 시험 중…");
  }
  // No badge while the question is still out: a standing is an answer, and
  // the button already says the asking is under way.
  paintSettingsStatus(statusEl, t("settings.apiRouters.testing", "연결 시험 중…"));

  try {
    const models = await fetchRouterModels({
      base_url: baseUrl,
      auth,
      key,
      models_path: modelsConfig.path,
      id_field: modelsConfig.id_field,
      context_field: modelsConfig.context_field,
      client_fingerprint: preset?.client_fingerprint,
      headers: preset?.headers,
    });
    if (generation !== routerProbeGeneration) return;
    testedModels = models ?? [];
    testedRouterGeneration = generation;

    if (tbody) tbody.replaceChildren();
    for (let i = 0; i < testedModels.length; i++) {
      const m = testedModels[i];
      const tr = document.createElement("tr");
      const tdCheck = document.createElement("td");
      const checkbox = document.createElement("input");
      checkbox.type = "checkbox";
      checkbox.className = "router-model-check";
      checkbox.dataset.id = m.id;
      checkbox.checked = true;
      checkbox.setAttribute("aria-label", m.id);
      tdCheck.appendChild(checkbox);
      tr.appendChild(tdCheck);

      const tdId = document.createElement("td");
      const code = document.createElement("code");
      code.textContent = m.id;
      tdId.appendChild(code);
      tr.appendChild(tdId);

      const tdCtx = document.createElement("td");
      tdCtx.textContent = m.context_length ? m.context_length.toLocaleString() : "-";
      tr.appendChild(tdCtx);

      if (tbody) tbody.appendChild(tr);
    }

    if (modelsSection) modelsSection.hidden = false;
    paintSettingsStatus(
      statusEl,
      t("settings.apiRouters.testSuccess", "연결 성공 — 사용할 모델을 선택하세요"),
      { standing: "connected" },
    );
    paintSettingsStanding(el("router-card-state"), "connected");
    updateRouterSaveButtonState();
  } catch (error) {
    if (generation !== routerProbeGeneration) return;
    // Ours first, the server's under the fold: what it said is evidence for a
    // support thread, not an instruction to the person reading this pane.
    paintSettingsStatus(
      statusEl,
      t("settings.apiRouters.testRefused", "연결하지 못했습니다 — 기본 URL과 API 키를 확인하세요."),
      { standing: "failed", raw: error?.message ?? String(error) },
    );
    paintSettingsStanding(el("router-card-state"), "failed");
    if (saveBtn) saveBtn.disabled = true;
    if (modelsSection) modelsSection.hidden = true;
  } finally {
    if (routerProbeInFlight === generation) {
      routerProbeInFlight = null;
      if (testBtn) {
        testBtn.disabled = false;
        testBtn.textContent = t("settings.apiRouters.testConnection", "연결 시험");
      }
      updateRouterSaveButtonState();
    }
  }
}

function updateRouterSaveButtonState() {
  const saveBtn = el("router-save-btn");
  const checked = document.querySelectorAll(".router-model-check:checked");
  if (saveBtn) {
    saveBtn.disabled = routerSaving || routerProbeInFlight !== null
      || testedRouterGeneration !== routerProbeGeneration || checked.length === 0
      || !el("router-name-input")?.value.trim();
  }
}

async function saveApiRouterEntry() {
  if (routerSaving || routerProbeInFlight !== null || testedRouterGeneration !== routerProbeGeneration) return;
  const generation = routerDraftGeneration;
  const saveBtn = el("router-save-btn");
  const statusEl = el("router-test-status");
  const nameInput = el("router-name-input");
  const urlInput = el("router-base-url-input");
  const keyInput = el("router-key-input");
  const select = el("router-preset-select");

  const name = nameInput?.value.trim() ?? "";
  const baseUrl = urlInput?.value.trim() ?? "";
  const key = keyInput?.value.trim() ?? "";
  const chosenId = select?.value ?? "custom";
  const preset = routerPresets.find((p) => p.id === chosenId);

  const checkedCheckboxes = [...document.querySelectorAll(".router-model-check:checked")];
  const checkedIds = checkedCheckboxes.map((cb) => cb.dataset.id);

  if (!name || !baseUrl || checkedIds.length === 0) return;

  // Each checked model with the window the probe read for it: a router lists
  // 32k and 1M models side by side, and one number for all of them either
  // never compacts the small ones or starves the large ones.
  const probedWindows = new Map(testedModels.map((m) => [m.id, m.context_length ?? null]));
  const models = checkedIds.map((id) => ({ id, context_window: probedWindows.get(id) ?? null }));

  if (saveBtn) saveBtn.disabled = true;
  routerSaving = true;
  try {
    // The row's identity — which variable and keychain item its key lives
    // under — is the backend's, decided at the row's first save and kept. The
    // pane says only which preset the row came from, if any, and its name.
    configuredRouters = (await invoke("save_api_router", {
      presetId: preset?.id ?? null,
      name,
      baseUrl,
      key: key || null,
      models,
      clientFingerprint: preset?.client_fingerprint ?? null,
      headers: preset?.headers ?? null,
    })) ?? [];

    paintConfiguredRouters();
    if (generation === routerDraftGeneration) {
      paintSettingsStatus(statusEl, t("settings.apiRouters.saved", "저장되었습니다"), {
        standing: "connected",
      });
    }
  } catch (error) {
    // Said beside the button that was pressed, not only in a toast: a key
    // this machine cannot keep is a thing to act on here.
    const reason = routerSaveRefusal(error);
    if (generation === routerDraftGeneration) {
      paintSettingsStatus(statusEl, reason, { standing: "failed" });
      paintSettingsStanding(el("router-card-state"), "failed");
    }
    showError(reason);
  } finally {
    routerSaving = false;
    updateRouterSaveButtonState();
  }
}

/* A refused save, in the reader's language when the backend named why.
 * `save_api_router` refuses as `{ kind, message }` — `failureOf`'s shape — so
 * the kind decides the words and never a sentence sniffed out of the message.
 * Off macOS there is no keychain for a router key: the row can still be saved
 * keyless, which is what the words say to do. */
function routerSaveRefusal(error) {
  const failure = failureOf(error);
  return failure.kind === "keychain-unavailable"
    ? t(
      "settings.apiRouters.keychainUnavailable",
      "이 컴퓨터에는 API 키를 보관할 키체인이 없어 저장하지 않았습니다. 키 없이 저장하려면 API 키 칸을 비우세요.",
    )
    : failure.message;
}

/* ---- which shortcut rows the column shows ----
 *
 * Orca's two are settings — `showTasksButton !== false` and
 * `showAutomationsButton !== false`, both on unless somebody turned them off —
 * and its row menu offers `Hide from sidebar`
 * (`App-BaqTRjaA.js:8683-8807`, docs/reverse/orca-ui-inventory.md 1-k).
 *
 * Both halves ship together, deliberately. A hide with no way back is not a
 * preference, it is a control that removes a destination for good — so the
 * gesture that puts a row away and the switch that brings it back are one
 * change. The answer is kept by the backend rather than here, for the reason
 * the theme is: it arrives in the boot report, so a row that was put away
 * never paints at all instead of vanishing a frame later. */
const SHORTCUT_ROWS = [
  { name: "tasks", id: "nav-tasks" },
  { name: "automations", id: "nav-automations" },
];
const navRows = document.querySelector(".nav-rows");
const hiddenShortcuts = new Set();

function paintShortcuts() {
  for (const row of SHORTCUT_ROWS) {
    const away = hiddenShortcuts.has(row.name);
    el(row.id).hidden = away;
    el(`show-${row.name}`).checked = !away;
  }
  // The strip itself goes when both rows are away, or the column keeps the
  // margin two invisible buttons used to occupy.
  navRows.hidden = SHORTCUT_ROWS.every((row) => hiddenShortcuts.has(row.name));
}

function setShortcutHidden(name, away) {
  if (away) hiddenShortcuts.add(name);
  else hiddenShortcuts.delete(name);
  paintShortcuts();
  void commitSetting("hidden_shortcuts", "set_shortcut_visibility", {
    name,
    visible: !away,
  });
}

for (const row of SHORTCUT_ROWS) {
  el(row.id).addEventListener("contextmenu", (event) => {
    event.preventDefault();
    openSidebarMenu(event.clientX, event.clientY, [
      {
        label: t("sidebar.hideFromSidebar", "사이드바에서 숨기기"),
        run: () => setShortcutHidden(row.name, true),
      },
    ]);
  });
  el(`show-${row.name}`).addEventListener("change", (event) => {
    setShortcutHidden(row.name, !event.target.checked);
  });
}

/* ---- how wide the columns are ----
 *
 * Orca's 280 and 350 are defaults a person drags, not constants (1-c). The
 * width lives on the root as the same custom property the grid reads, so a
 * drag is one property write rather than a re-layout the window has to
 * orchestrate.
 *
 * The bounds are the backend's, repeated here because a grip that can be
 * dragged past a width the backend will clamp shows the person one number and
 * stores another. `the_two_ends_of_a_panel_drag_agree_on_the_bounds` holds
 * the two copies together. */
const PANEL_BOUNDS = {
  sidebar: { min: 180, max: 560 },
  aside: { min: 200, max: 640 },
};

const panelWidths = { sidebar: 280, aside: 350 };

/* One step of a keyboard nudge. A grip is a control, and a control that only
 * a mouse can reach is one a person on a keyboard cannot use at all. */
const PANEL_NUDGE_PX = 16;

const RIGHT_SIDEBAR_MIN_STAGE_WIDTH = 320;
const RIGHT_SIDEBAR_MIN_LEAF_WIDTH = 260;

function responsiveAsideMax() {
  const workbench = el("workbench");
  const width = workbench.getBoundingClientRect().width || window.innerWidth;
  const measuredSidebar = workbench.querySelector(".threads")?.getBoundingClientRect().width ?? 0;
  const sidebar = folded.sidebar ? 0 : (measuredSidebar || panelWidths.sidebar);
  const leaves = Math.max(1, stageGroups().length);
  const stageReserve = Math.max(
    RIGHT_SIDEBAR_MIN_STAGE_WIDTH,
    Math.min(900, leaves * RIGHT_SIDEBAR_MIN_LEAF_WIDTH),
  );
  return Math.min(
    PANEL_BOUNDS.aside.max,
    Math.max(PANEL_BOUNDS.aside.min, Math.floor(width - sidebar - stageReserve)),
  );
}

function renderedPanelWidth(side) {
  return side === "aside"
    ? Math.min(panelWidths.aside, responsiveAsideMax())
    : panelWidths.sidebar;
}

function applyPanelWidth(side) {
  const rendered = renderedPanelWidth(side);
  document.documentElement.style.setProperty(
    side === "sidebar" ? "--sidebar-width" : "--aside-width",
    `${rendered}px`,
  );
  const grip = el(side === "sidebar" ? "grip-sidebar" : "grip-aside");
  grip.setAttribute("aria-valuenow", String(rendered));
  if (side === "aside") {
    grip.setAttribute("aria-valuemax", String(responsiveAsideMax()));
    grip.dataset.clamped = String(rendered !== panelWidths.aside);
  }
}

function refreshResponsivePanelWidths() {
  applyPanelWidth("aside");
}

/* Set one column's width, held inside the bounds.
 *
 * Nothing is told about the new size on purpose. The terminals are measured
 * in cells and a narrower column hands the stage more of them, but the stage
 * is already under a `ResizeObserver` — telling it here as well would resize
 * every pty twice for one drag, once eagerly and once when the observer
 * settles. */
function setPanelWidth(side, px) {
  const { min } = PANEL_BOUNDS[side];
  const max = side === "aside" ? responsiveAsideMax() : PANEL_BOUNDS[side].max;
  const kept = Math.round(Math.min(max, Math.max(min, px)));
  if (kept === panelWidths[side]) return;
  panelWidths[side] = kept;
  applyPanelWidth(side);
}

/* Written when the drag ends, not while it runs: a width being dragged passes
 * through every pixel between where it started and where it stopped, and
 * none of those is a decision worth a file write. */
function rememberPanelWidth(side) {
  void commitSetting(`panel_widths.${side}`, "set_panel_width", {
    side,
    width: panelWidths[side],
  });
  // 폭을 자기 손으로 정한 사람에게 폭을 정하는 법을 가르치는 팁은 방해다.
  markFirstRun("shaped_sidebar");
}

/* Dragging a column edge. The pointer is captured so the drag survives the
 * cursor crossing the terminal, which swallows mouse events of its own. */
function gripDrag(side, grip) {
  grip.setAttribute("aria-valuemin", String(PANEL_BOUNDS[side].min));
  grip.setAttribute("aria-valuemax", String(PANEL_BOUNDS[side].max));
  grip.setAttribute("aria-valuenow", String(panelWidths[side]));
  grip.addEventListener("pointerdown", (event) => {
    if (event.button !== 0) return;
    event.preventDefault();
    grip.setPointerCapture(event.pointerId);
    grip.classList.add("is-dragging");
    // The fold's animation is on the same property this drag writes, so it is
    // switched off for the length of the drag.
    el("workbench").classList.add("is-gripping");
  });
  grip.addEventListener("pointermove", (event) => {
    if (!grip.hasPointerCapture(event.pointerId)) return;
    // Measured against the window edge the column is anchored to, so the grip
    // stays under the pointer instead of drifting by however far the drag
    // started from the edge.
    const width =
      side === "sidebar"
        ? event.clientX - el("workbench").getBoundingClientRect().left
        : el("workbench").getBoundingClientRect().right - event.clientX;
    setPanelWidth(side, width);
  });
  const release = (event) => {
    if (!grip.hasPointerCapture(event.pointerId)) return;
    grip.releasePointerCapture(event.pointerId);
    grip.classList.remove("is-dragging");
    el("workbench").classList.remove("is-gripping");
    rememberPanelWidth(side);
  };
  grip.addEventListener("pointerup", release);
  grip.addEventListener("pointercancel", release);
  grip.addEventListener("keydown", (event) => {
    // Left narrows the column and right widens it — for the file panel that
    // means the arrows are reversed, because its edge faces the other way.
    const step = event.key === "ArrowLeft" ? -1 : event.key === "ArrowRight" ? 1 : 0;
    if (step === 0) return;
    event.preventDefault();
    setPanelWidth(side, panelWidths[side] + step * PANEL_NUDGE_PX * (side === "sidebar" ? 1 : -1));
    rememberPanelWidth(side);
  });
}

gripDrag("sidebar", el("grip-sidebar"));
gripDrag("aside", el("grip-aside"));

window.addEventListener("resize", refreshResponsivePanelWidths);
if (typeof ResizeObserver !== "undefined") {
  const panelWidthObserver = new ResizeObserver(refreshResponsivePanelWidths);
  panelWidthObserver.observe(el("workbench"));
}

/* The language row. Orca puts one here too, and its own key for it is
 * `settings.appearance.language.title` with a `.system` option — measured,
 * not guessed (docs/reverse/orca-ui-inventory.md 1-h). Native names, the way
 * Orca prints them: a person looking for their language is looking for the
 * word they call it, not for its English name. */
const localePicker = el("app-locale");

function paintLocalePicker() {
  localePicker.replaceChildren();
  for (const choice of LOCALES) {
    const option = document.createElement("option");
    option.value = choice.code;
    // The system row is the one label that IS translated: it names a
    // behaviour rather than a language.
    option.textContent =
      choice.code === "system"
        ? t("settings.appearance.language.system", "시스템 설정")
        : choice.name;
    option.selected = choice.code === locale;
    localePicker.appendChild(option);
  }
}

/* The open documents, redrawn in the language now in force.
 *
 * `updateStage` is the one path that repaints a visible document and it is the
 * right one to ask — but `paintFileView` and `paintDiffView` both end by
 * sending the body back to the top. That is what you want when a tab is
 * opened, and not what you want when the only thing that changed is the
 * language: somebody halfway down a long diff keeps their place. */
function repaintDocumentsForLocale() {
  const scrolled = new Map();
  for (const body of document.querySelectorAll(".file-body, .diff-body")) {
    scrolled.set(body, body.scrollTop);
  }
  updateStage();
  for (const [body, top] of scrolled) body.scrollTop = top;
}

/* Change the language, everywhere, now.
 *
 * The static markup is re-read and every surface that builds its own text is
 * repainted — a language that only took effect on the next boot would be a
 * setting nobody trusts. */
function setLocale(code) {
  const { refresh = true, persist = true } = arguments[1] ?? {};
  locale = code;
  document.documentElement.lang = code === "system" ? systemLocale : code;
  applyLocale();
  paintLocalePicker();
  paintExplorerFilters();
  // The treatment names are translated, unlike the language names — so this
  // picker's own labels change with the language.
  paintThemePicker();
  paintSettingsKeys();
  paintSettingsHealth();
  paintUpdateNotice();
  paintUpdatePane();
  paintEditingPrefs();
  paintTermPrefs();
  paintOpenInApplications();
  paintBrowserPrefs();
  // Agent pills, permission badges and row notes are built dynamically, so
  // `applyLocale` cannot reach their source keys after they exist.
  paintAgents();
  paintUsageSegments();
  paintUsagePanel();
  paintStatsUsage();
  // After `applyLocale`, which writes the words: this puts the key back on the
  // end of them in the language just chosen.
  paintChordTitles();
  paintCommandLabel();
  // `refreshChrome` rather than `renderTabs`: it draws the tabs too, and it is
  // the only thing that reaches a lane row's state word and close control.
  refreshChrome();
  relabelTerminalViews();
  paintStagePlaceholder();
  // The documents on screen name their own mode, their save state and their
  // empty cases.
  repaintDocumentsForLocale();
  // The schedule screen builds its own words too: the pickers' options, each
  // row's next-run line, and the sentence under the form.
  paintAutoPickers();
  if (!el("flow-console").hidden) paintFlowConsole();
  // The TypeSafe card's switch line and the Jev dashboard build their words
  // from the backend's answer and the features' template — each feature's
  // name and one-sentence tip — so `applyLocale` has no key on them to sweep.
  if (typesafeState) paintTypeSafe(typesafeState);
  paintAutomations();
  // 정리 목록은 행을 직접 짓는다 — 칩과 필과 버튼의 말이 전부 여기서 나오므로,
  // 언어가 바뀌면 다시 지어야 한다. 열려 있지 않아도 사이드바의 한 줄은 이
  // 함수가 그린다.
  paintCleanup();
  // 작업판 GitHub의 필도 마찬가지다: 고른 상태의 낱말과 그 필의 aria 라벨이
  // 여기서 나온다. 속성은 `say`가 되돌려 놓지 못하는 자리다.
  paintGithubFilters();
  // 소스 제어의 빈 줄(필터가 세운 말 포함)과 파일 필터 토글의 팁도 같은
  // 이유로 여기서 다시 짓는다 — 팁은 속성이고, 빈 줄의 말은 paintScm이 고른다.
  paintScm();
  paintScmFilter();
  // Checks 패널의 동적인 말들(요약 카운트 "3 통과", 상태 배지, updated 줄,
  // 충돌 스트립의 앞말)도 paintChecks가 고른다 — 마지막 보고에서 다시 짓는다.
  paintChecks();
  // SSH 대상 카드의 편집·제거는 아이콘 버튼이라, 그 말이 전부 data-tip과
  // aria-label에 있다. 같은 이유로 여기서 다시 짓는다.
  paintSshTargets();
  if (autoDraft) {
    paintAutoSchedule();
    // The agent list too: its first option is a translated word, and the id
    // being held — which may be one this build has never heard of — has to
    // survive the rebuild rather than fall back to the default.
    paintAutoAgents(el("auto-agent").value || null);
  }
  // And the modal, if one is up. The lane behind it is blocked until it is
  // answered, so it is the last surface that can afford to be left behind.
  paintPermission();
  // 근거 화면도 제 말을 직접 짓는다 — 상태 딱지, 신선도, 그리고 「시험 결과가
  // 아니다」라는 유보 문장까지. 떠 있지 않으면 아무것도 하지 않는다.
  paintWorktreeEvidence();
  // 그리고 첫 실행 마법사, 떠 있다면. `say`가 문장은 되돌려 놓지만 단계 표시의
  // aria 라벨은 속성이고, 속성은 아무도 다시 쓰지 않는다.
  if (!el("onb-scrim").hidden) paintOnboarding();
  if (refresh) {
    refreshScm().catch(() => {});
    refreshWorktrees();
  }
  if (persist) void commitSetting("locale", "set_locale", { code });
}

el("keys-search").addEventListener("input", (event) => {
  keyQuery = event.target.value;
  paintSettingsKeys();
});

localePicker.addEventListener("change", () => setLocale(localePicker.value));

/* ---- the terminal's own settings (1-en) ----
 *
 * One typed record for the terminal controls this runtime can actually honour.
 * Orca exposes more than twenty `terminal*` options; copying controls with no
 * consumer would only create settings that lie.
 *
 * Renderer-owned fields apply through this record's live consumers. Scrollback
 * stays Rust-owned, where the terminal grid enforces it. The window keeps no
 * second default copy of either the values or their allowed ranges.
 *
 * The defaults here are what the window already shipped, so this object on a
 * machine with no settings file describes exactly the terminal that machine
 * already had. They are stated once, in Rust, and arrive in the boot report —
 * a second copy here would be the pair that drifts. */
/* ---- Terminal → Manage Sessions -----------------------------------------
 *
 * Orca lists the PTY daemon's sessions. ZeroCode does not have a shared PTY
 * daemon: the native process-owned terminal pool is the authority, so this
 * surface asks that pool directly and uses the existing `term:exited` event
 * to retire panes, tabs, agents and views through their one normal door. */
let managedTerminalSessions = [];
let terminalSessionsLoading = false;
let terminalSessionsProblem = "";
let terminalSessionsGeneration = 0;
let terminalSessionsRefreshDeferred = false;

function managedTerminalSessionLabel(session) {
  if (session.term === FLOAT_TERM) {
    return t("settings.terminal.sessionsFloating", "플로팅 터미널");
  }
  const holder = tabs.find(
    (tab) => tab.kind === "term" && paneLeaves(tab.layout).includes(session.term),
  );
  return holder
    ? tabLabel(holder)
    : t("settings.terminal.sessionsTerminal", "터미널 {{term}}", { term: session.term });
}

function managedTerminalSessionMeta(session) {
  const facts = [];
  if (session.agent) facts.push(agentName(session.agent));
  else if (session.program) facts.push(session.program);
  facts.push(
    session.running === true
      ? t("settings.terminal.sessionsRunning", "실행 중")
      : session.running === false
        ? t("settings.terminal.sessionsIdle", "셸 대기 중")
        : t("settings.terminal.sessionsUnknown", "상태를 확인할 수 없음"),
  );
  return facts.join(" · ");
}

function paintTerminalSessions() {
  const host = el("term-session-list");
  host.replaceChildren();
  host.setAttribute("aria-busy", String(terminalSessionsLoading));
  el("term-session-count").textContent = String(managedTerminalSessions.length);
  el("term-session-empty").hidden = managedTerminalSessions.length > 0
    || terminalSessionsLoading;
  el("term-session-refresh").disabled = terminalSessionsLoading;
  el("term-session-end-all").disabled = terminalSessionsLoading
    || managedTerminalSessions.length === 0;
  el("term-session-status").textContent = terminalSessionsLoading
    ? t("settings.terminal.sessionsLoading", "세션을 확인하는 중…")
    : terminalSessionsProblem;

  for (const session of managedTerminalSessions) {
    const label = managedTerminalSessionLabel(session);
    const row = document.createElement("div");
    row.className = "settings-command-row";
    row.setAttribute("role", "listitem");

    const copy = document.createElement("span");
    copy.className = "settings-command-copy";
    const name = document.createElement("span");
    name.className = "settings-command-name";
    name.textContent = label;
    const meta = document.createElement("span");
    meta.className = "settings-command-meta";
    meta.textContent = managedTerminalSessionMeta(session);
    copy.append(name, meta);

    const end = document.createElement("button");
    end.className = "settings-command-delete";
    end.type = "button";
    end.dataset.termSession = String(session.term);
    end.dataset.tip = t("settings.terminal.sessionsEndOne", "세션 종료");
    end.setAttribute("aria-label", `${label} — ${end.dataset.tip}`);
    end.innerHTML = icon("trash");
    end.addEventListener("click", () => requestEndTerminalSession(session));
    row.append(copy, end);
    host.appendChild(row);
  }
}

async function refreshTerminalSessions() {
  if (terminalSessionsLoading) {
    terminalSessionsRefreshDeferred = true;
    return;
  }
  terminalSessionsRefreshDeferred = false;
  const generation = ++terminalSessionsGeneration;
  terminalSessionsLoading = true;
  terminalSessionsProblem = "";
  paintTerminalSessions();
  try {
    const sessions = await invoke("terminal_sessions");
    if (generation !== terminalSessionsGeneration) return;
    managedTerminalSessions = Array.isArray(sessions) ? sessions : [];
  } catch (error) {
    if (generation !== terminalSessionsGeneration) return;
    terminalSessionsProblem = t(
      "settings.terminal.sessionsLoadFailed",
      "세션을 불러오지 못했습니다.",
    );
    showError(error);
  } finally {
    terminalSessionsLoading = false;
    paintTerminalSessions();
    const retry = terminalSessionsRefreshDeferred;
    terminalSessionsRefreshDeferred = false;
    if (retry && settingsPane === "terminal" && !settingsView.hidden) {
      void refreshTerminalSessions();
    }
  }
}

async function runTerminalSessionMutation(command, args) {
  if (terminalSessionsLoading) return;
  ++terminalSessionsGeneration;
  terminalSessionsLoading = true;
  terminalSessionsProblem = "";
  paintTerminalSessions();
  try {
    await invoke(command, args);
    terminalSessionsLoading = false;
    await refreshTerminalSessions();
  } catch (error) {
    terminalSessionsLoading = false;
    terminalSessionsProblem = t(
      "settings.terminal.sessionsEndFailed",
      "세션을 종료하지 못했습니다.",
    );
    paintTerminalSessions();
    showError(error);
  }
}

async function requestEndTerminalSession(session) {
  const label = managedTerminalSessionLabel(session);
  const accepted = await askConfirm({
    title: t("settings.terminal.sessionsEndOneTitle", "터미널 세션을 종료할까요?"),
    body: t(
      "settings.terminal.sessionsEndOneBody",
      "{{name}}에서 실행 중인 프로세스가 중지됩니다.",
      { name: label },
    ),
    confirm: t("settings.terminal.sessionsEndOne", "세션 종료"),
    deny: t("app.cancel", "취소"),
    danger: true,
  });
  if (accepted === true) {
    await runTerminalSessionMutation("end_terminal_session", { term: session.term });
  }
}

async function requestEndAllTerminalSessions() {
  const count = managedTerminalSessions.length;
  if (count === 0) return;
  const accepted = await askConfirm({
    title: t("settings.terminal.sessionsEndAllTitle", "모든 터미널 세션을 종료할까요?"),
    body: t(
      "settings.terminal.sessionsEndAllBody",
      "열려 있는 터미널 {{count}}개의 프로세스가 모두 중지됩니다.",
      { count },
    ),
    confirm: t("settings.terminal.sessionsEndAll", "모두 종료"),
    deny: t("app.cancel", "취소"),
    danger: true,
  });
  if (accepted === true) {
    await runTerminalSessionMutation("end_all_terminal_sessions", {});
  }
}

function forgetManagedTerminalSession(term) {
  // A list read may already be in flight with this terminal in its snapshot.
  // Retire that generation before repainting, then re-read while the pane is
  // visible so a stale response cannot resurrect the exited process.
  ++terminalSessionsGeneration;
  const next = managedTerminalSessions.filter((session) => session.term !== term);
  if (next.length !== managedTerminalSessions.length) {
    managedTerminalSessions = next;
    paintTerminalSessions();
  }
  if (settingsPane === "terminal" && !settingsView.hidden) {
    void refreshTerminalSessions();
  }
}

el("term-session-refresh").addEventListener("click", () => {
  void refreshTerminalSessions();
});
el("term-session-end-all").addEventListener("click", () => {
  void requestEndAllTerminalSessions();
});

let termPrefs = null;
let termPrefsSpec = null;
const SETUP_SCRIPT_LAUNCH_MODES = Object.freeze([
  "new-tab",
  "split-vertical",
  "split-horizontal",
]);
let setupScriptLaunchMode = "new-tab";
let termScrollbackCustomRequested = false;
let termScrollbackCanonical = null;
let termScrollbackDraft = "";
let terminalKeyboardLayoutCategory = "unknown";
let terminalKeyboardLayoutProbe = 0;
let terminalWindowsStatus = null;
let terminalWindowsStatusProbe = 0;
let terminalThemeImportPreview = null;
let terminalThemeImportBusy = false;
let terminalThemeImportReturnFocus = null;
let ghosttyImportPreview = null;
let ghosttyImportBusy = false;
let ghosttyImportApplied = false;
let ghosttyImportReturnFocus = null;
let ghosttyImportApplyError = "";
let ghosttyImportRequest = 0;

function normalizedSetupScriptLaunchMode(mode) {
  return SETUP_SCRIPT_LAUNCH_MODES.includes(mode) ? mode : "new-tab";
}

function paintSetupScriptLaunchMode() {
  for (const button of el("setup-script-launch-mode").querySelectorAll(
    "[data-setup-launch-mode]",
  )) {
    const active = button.dataset.setupLaunchMode === setupScriptLaunchMode;
    button.classList.toggle("is-active", active);
    button.setAttribute("aria-pressed", String(active));
  }
}

function setSetupScriptLaunchMode(mode) {
  setupScriptLaunchMode = normalizedSetupScriptLaunchMode(mode);
  paintSetupScriptLaunchMode();
  void commitSetting(
    "setup_script_launch_mode",
    "set_setup_script_launch_mode",
    { mode: setupScriptLaunchMode },
  );
}

el("setup-script-launch-mode").addEventListener("click", (event) => {
  const button = event.target.closest?.("[data-setup-launch-mode]");
  if (!button || !event.currentTarget.contains(button)) return;
  setSetupScriptLaunchMode(button.dataset.setupLaunchMode);
});

function macOptionAsAltLabel(mode) {
  if (mode === "true") return t("settings.terminal.optionAsAltBoth", "양쪽");
  if (mode === "left") return t("settings.terminal.optionAsAltLeft", "왼쪽");
  if (mode === "right") return t("settings.terminal.optionAsAltRight", "오른쪽");
  if (mode === "false") return t("settings.terminal.optionAsAltOff", "끄기");
  return t("settings.terminal.optionAsAltAuto", "자동");
}

function macOptionAsAltStatus() {
  if (termPrefs?.mac_option_as_alt !== "auto") return "";
  if (terminalKeyboardLayoutCategory === "us") {
    return t(
      "settings.terminal.optionAsAltAutoOn",
      "자동 — 현재 US 입력 소스에서는 Alt/Esc를 보냅니다.",
    );
  }
  if (terminalKeyboardLayoutCategory === "non_us") {
    return t(
      "settings.terminal.optionAsAltAutoOff",
      "자동 — 현재 입력 소스에서는 문자 조합을 유지합니다.",
    );
  }
  return t(
    "settings.terminal.optionAsAltAutoUnknown",
    "자동 — 입력 소스를 읽지 못해 문자 조합을 유지합니다.",
  );
}

async function refreshTerminalKeyboardLayout() {
  if (!usesCommandModifier) return;
  const probe = ++terminalKeyboardLayoutProbe;
  let category = "unknown";
  try {
    const report = await invoke("terminal_keyboard_layout");
    if (["us", "non_us"].includes(report?.category)) category = report.category;
  } catch {
    // Unknown is the safe answer: keep composition instead of turning an
    // unreadable keyboard layout into terminal control bytes.
  }
  if (probe !== terminalKeyboardLayoutProbe) return;
  terminalKeyboardLayoutCategory = category;
  if (termPrefs !== null) paintTermPrefs();
}

function windowsPowerShellImplementationLabel(value) {
  if (value === "powershell.exe") {
    return t("settings.terminal.powerShellWindows", "Windows PowerShell");
  }
  if (value === "pwsh.exe") {
    return t("settings.terminal.powerShell7", "PowerShell 7+");
  }
  return t("settings.terminal.powerShellAuto", "자동");
}

function windowsTerminalShellLabel(value) {
  if (value === "cmd.exe") {
    return t("settings.terminal.windowsShellCommandPrompt", "명령 프롬프트");
  }
  if (value === "git-bash") {
    return t("settings.terminal.windowsShellGitBash", "Git Bash");
  }
  return t("settings.terminal.windowsShellPowerShell", "PowerShell");
}

function windowsTerminalShellStatusText() {
  if (terminalWindowsStatus === null) {
    return t(
      "settings.terminal.windowsShellChecking",
      "설치된 Windows 셸을 확인하고 있습니다.",
    );
  }
  if (termPrefs?.windows_shell === "git-bash" && !terminalWindowsStatus.git_bash_available) {
    return t(
      "settings.terminal.windowsShellGitBashUnavailable",
      "Git Bash를 찾지 못해 새 터미널은 PowerShell로 대체됩니다.",
    );
  }
  return "";
}

function windowsPowerShellStatusText() {
  const selected = termPrefs?.windows_powershell_implementation ?? "auto";
  if (terminalWindowsStatus === null) {
    return t(
      "settings.terminal.powerShellChecking",
      "PowerShell 7+ 설치 여부를 확인하고 있습니다.",
    );
  }
  if (selected === "auto") {
    return terminalWindowsStatus.pwsh_available
      ? t(
        "settings.terminal.powerShellAuto7",
        "자동 — 새 터미널은 설치된 PowerShell 7+를 사용합니다.",
      )
      : t(
        "settings.terminal.powerShellAutoWindows",
        "자동 — PowerShell 7+가 설치될 때까지 Windows PowerShell을 사용합니다.",
      );
  }
  if (selected === "pwsh.exe" && !terminalWindowsStatus.pwsh_available) {
    return t(
      "settings.terminal.powerShell7Unavailable",
      "PowerShell 7+를 찾지 못해 새 터미널은 Windows PowerShell로 대체됩니다.",
    );
  }
  return "";
}

async function refreshTerminalWindowsStatus() {
  if (!usesWindowsPlatform || isPopout) return;
  const probe = ++terminalWindowsStatusProbe;
  let report = { supported: true, pwsh_available: false, git_bash_available: false };
  try {
    const next = await invoke("terminal_windows_status");
    if (next?.supported === true) {
      report = {
        supported: true,
        pwsh_available: next.pwsh_available === true,
        git_bash_available: next.git_bash_available === true,
      };
    }
  } catch {
    // An unreadable PATH is the conservative answer: do not offer an
    // executable whose existence the native side could not prove.
  }
  if (probe !== terminalWindowsStatusProbe) return;
  terminalWindowsStatus = report;
  if (termPrefs !== null) paintTermPrefs();
}

window.addEventListener("focus", () => {
  if (termPrefs === null) return;
  void refreshTerminalKeyboardLayout();
  void refreshTerminalWindowsStatus();
});

function terminalBoldWeight(weight, spec) {
  return Math.min(
    spec.weight.max,
    Math.max(spec.bold_weight_floor, weight + spec.bold_weight_offset),
  );
}

function terminalLineHeightControlValue(leading, spec) {
  const base = spec?.leading_base ?? 1;
  return Number((leading / base).toFixed(2));
}

function terminalLineHeightStorageValue(lineHeight, spec) {
  const base = spec?.leading_base ?? 1;
  return Number((lineHeight * base).toFixed(4));
}

function terminalDefaultFontFamily(spec) {
  const defaults = spec?.font_family_defaults;
  if (usesWindowsPlatform) return defaults?.windows ?? "Cascadia Mono";
  if (usesCommandModifier) return defaults?.macos ?? "SF Mono";
  return defaults?.linux ?? "DejaVu Sans Mono";
}

function terminalFontFamilyChoice(family, spec) {
  return String(family ?? "").trim() || terminalDefaultFontFamily(spec);
}

function terminalLigatureLabel(mode) {
  if (mode === "on") return t("settings.terminal.ligatureOn", "켜기");
  if (mode === "off") return t("settings.terminal.ligatureOff", "끄기");
  return t("settings.terminal.ligatureAuto", "자동");
}

function terminalFontHasKnownLigatures(family, spec) {
  const primary = terminalFontFamilyChoice(family, spec)
    .split(",", 1)[0]
    .trim()
    .replace(/^(['"])(.*)\1$/, "$2")
    .toLocaleLowerCase();
  return (spec?.ligature_font_tokens ?? []).some((token) => primary.includes(token));
}

function terminalLigaturesEnabled(mode, family, spec) {
  if (mode === "on") return true;
  if (mode === "off") return false;
  return terminalFontHasKnownLigatures(family, spec);
}

function terminalLigatureStatus(mode, family, spec) {
  if (mode === "on") {
    return t(
      "settings.terminal.ligatureAlwaysOn",
      "항상 켭니다. 합자를 제공하지 않는 글꼴은 그대로 표시됩니다.",
    );
  }
  if (mode === "off") {
    return t(
      "settings.terminal.ligatureAlwaysOff",
      "합자를 제공하는 글꼴에서도 항상 끕니다.",
    );
  }
  const font = terminalFontFamilyChoice(family, spec);
  return terminalLigaturesEnabled(mode, family, spec)
    ? t("settings.terminal.ligatureAutoOn", "자동 — {{font}}에서 켜집니다.", { font })
    : t("settings.terminal.ligatureAutoOff", "자동 — {{font}}에서 꺼집니다.", { font });
}

function normalizedTerminalDividerColor(kind, color, spec) {
  const fallback = kind === "divider_color_light"
    ? spec?.divider_color_defaults?.light
    : spec?.divider_color_defaults?.dark;
  return normalizedHexColor(color, fallback ?? "#000000");
}

const TERMINAL_BASE_COLOR_VARIABLES = Object.freeze({
  background: "--term-screen-bg",
  foreground: "--term-screen-fg",
  cursor: "--term-cursor",
  cursorAccent: "--term-cursor-accent",
  selectionBackground: "--term-selection-bg",
  selectionForeground: "--term-selection-fg",
});

let terminalPaletteSignature = "";

function terminalThemeUsesLightVariant() {
  return document.documentElement.dataset.theme === "light" &&
    termPrefs?.use_separate_light_theme === true;
}

function customTerminalTheme(selection) {
  const prefix = termPrefsSpec?.custom_theme_selection_prefix;
  if (!prefix || !selection?.startsWith(prefix)) return null;
  const id = selection.slice(prefix.length);
  return (termPrefs?.custom_themes ?? []).find((theme) => theme.id === id) ?? null;
}

function terminalThemePalette() {
  const catalog = termPrefsSpec?.terminal_themes;
  if (!catalog || termPrefs === null) return null;
  const light = terminalThemeUsesLightVariant();
  const fallback = light
    ? termPrefsSpec.theme_defaults.light
    : termPrefsSpec.theme_defaults.dark;
  const selected = light ? termPrefs.theme_light : termPrefs.theme_dark;
  const custom = customTerminalTheme(selected);
  if (custom) return { ...(catalog[fallback] ?? {}), ...(custom.terminal ?? {}) };
  return catalog[selected] ?? catalog[fallback] ?? null;
}

function terminalAnsiColorKeys() {
  const groups = termPrefsSpec?.color_override_groups ?? [];
  return groups
    .filter((group) => group.id === "normal" || group.id === "bright")
    .flatMap((group) => group.keys);
}

function applyTerminalPalette() {
  const palette = terminalThemePalette();
  if (palette === null) return false;
  const root = document.documentElement;
  const colors = { ...palette, ...(termPrefs.color_overrides ?? {}) };
  for (const [key, variable] of Object.entries(TERMINAL_BASE_COLOR_VARIABLES)) {
    root.style.setProperty(variable, colors[key]);
  }
  terminalAnsiColorKeys().forEach((key, index) => {
    root.style.setProperty(`--term-${index}`, colors[key]);
  });
  root.style.setProperty(
    "--term-divider-color-active",
    terminalThemeUsesLightVariant()
      ? termPrefs.divider_color_light
      : termPrefs.divider_color_dark,
  );
  const signature = JSON.stringify(colors);
  if (signature !== terminalPaletteSignature) {
    terminalPaletteSignature = signature;
    forgetPalette();
  }
  return true;
}

/* Put the record on the root, where the tokens are bound.
 *
 * Every screen reads the visual fields through the cascade, so no individual
 * view is told about font, leading, weight, or cursor blink. Interaction fields
 * are read directly by the wheel, selection, and pane-focus paths. Scrollback
 * remains backend-owned.
 *
 * The cell metrics change with the font, and the pty is sized from MEASURED
 * metrics — so every view has to re-measure and every shell has to be retold
 * its grid, or the child keeps writing for a screen that is no longer that
 * shape. That is the one thing this cannot leave to the cascade. */
function applyTermPrefs(previous = null) {
  if (termPrefs === null) return;
  const root = document.documentElement;
  root.style.setProperty("--term-font-size", `${termPrefs.font_size}px`);
  root.style.setProperty(
    "--term-font-choice",
    cssFontFamilyChoice(terminalFontFamilyChoice(termPrefs.font_family, termPrefsSpec)),
  );
  root.dataset.termLigatures = terminalLigaturesEnabled(
    termPrefs.ligatures,
    termPrefs.font_family,
    termPrefsSpec,
  ) ? "on" : "off";
  root.style.setProperty("--term-leading", String(termPrefs.leading));
  root.style.setProperty("--term-weight", String(termPrefs.weight));
  root.dataset.termCursorStyle = termPrefs.cursor_style;
  root.style.setProperty("--term-cursor-opacity", String(termPrefs.cursor_opacity));
  root.style.setProperty("--term-padding-x", `${termPrefs.padding_x}px`);
  root.style.setProperty("--term-padding-y", `${termPrefs.padding_y}px`);
  root.style.setProperty(
    "--term-inactive-pane-opacity",
    String(termPrefs.inactive_pane_opacity),
  );
  root.style.setProperty("--term-divider-thickness", `${termPrefs.divider_thickness_px}px`);
  root.style.setProperty("--term-divider-color-dark", termPrefs.divider_color_dark);
  root.style.setProperty("--term-divider-color-light", termPrefs.divider_color_light);
  applyTerminalPalette();
  if (termPrefsSpec !== null) {
    root.style.setProperty(
      "--term-bold-weight",
      String(terminalBoldWeight(termPrefs.weight, termPrefsSpec)),
    );
    root.style.setProperty(
      "--term-divider-hit-size",
      `${termPrefs.divider_thickness_px + termPrefsSpec.divider_hit_padding_px}px`,
    );
    // Half the hit: Orca's `--divider-extension` is `hitSize / 2` — the
    // "extension amount [that] lets ::after reach the center of perpendicular
    // dividers so intersecting splits visually connect" (`applyDividerStyles`,
    // src/renderer/src/lib/pane-manager/pane-divider.ts).
    root.style.setProperty(
      "--term-divider-extension",
      `${(termPrefs.divider_thickness_px + termPrefsSpec.divider_hit_padding_px) / 2}px`,
    );
  }
  // An attribute rather than a variable: it turns a rule off, and a rule is
  // not a value.
  if (termPrefs.cursor_blink) delete root.dataset.termBlink;
  else root.dataset.termBlink = "off";
  if (termPrefs.hide_mouse_while_typing !== true) {
    for (const view of [stageView, floatView, ...termViews.values()]) {
      view.host.classList.remove("is-pointer-hidden");
    }
  }
  const metricsChanged = previous === null || [
    "font_size",
    "font_family",
    "leading",
    "weight",
    "padding_x",
    "padding_y",
  ]
    .some((key) => previous[key] !== termPrefs[key]);
  if (metricsChanged) resizeAllTerminals();
  // The editor font is BASE plus zoom, and the base just moved — recompute
  // here rather than teaching the zoom path about terminal settings.
  if (previous === null || previous.font_size !== termPrefs.font_size) {
    applyEditorFontZoom();
  }
}

/* Re-measure every screen and tell every shell its new grid.
 *
 * One door, because the font size reaches the pty only through the cell box:
 * the window measures a row out of the live DOM and hands the pty the columns
 * and rows that fit. A view that did not re-measure would keep the old cell
 * and ask for a grid that does not match what is being drawn — the exact
 * misalignment the measurement exists to prevent, arriving from settings
 * instead of from a font loading late. */
function resizeAllTerminals() {
  for (const [term, view] of termViews) {
    view.measure();
    resizeTermTab(term);
  }
  floatView.measure();
  if (!termFloat.hidden) {
    const { rows, cols } = floatView.gridSize();
    invoke("term_resize", { term: FLOAT_TERM, rows, cols }).catch(() => {});
  }
  stageView.measure();
  resizeStageLane();
}

function optionRow(select, values, chosen, label) {
  select.replaceChildren();
  for (const value of values) {
    const option = document.createElement("option");
    option.value = String(value);
    option.textContent = label(value);
    option.selected = value === chosen;
    select.appendChild(option);
  }
}

function paintNumberSetting(id, spec, value) {
  const field = el(id);
  if (spec) {
    field.min = String(spec.min);
    field.max = String(spec.max);
    field.step = String(spec.step);
  }
  field.value = String(value);
}

function cursorStyleLabel(style) {
  if (style === "bar") return t("settings.terminal.cursorBar", "막대");
  if (style === "underline") return t("settings.terminal.cursorUnderline", "밑줄");
  return t("settings.terminal.cursorBlock", "블록");
}

/* The measured 95-column upper bound from
 * docs/measurements/pty-footprint-verdict-20260831.md §2.3. A full-width
 * stored row occupies one 5,120-byte macOS allocation; the complete per-pane
 * bound also owns two 47-row screens and two rings whose combined slot is 28
 * bytes. Keeping the arithmetic here means presets and custom drafts cannot
 * drift into different claims. Full-width rows remain this worst case after
 * short scrollback rows are trimmed. */
const SCROLLBACK_ROW_BYTES_AT_95_COLUMNS = 5_120;
const SCROLLBACK_SCREEN_ROWS_AT_MEASUREMENT = 47 * 2;
const SCROLLBACK_RING_BYTES_PER_SLOT = 28;
const BYTES_PER_MEBIBYTE = 1024 ** 2;

function scrollbackWorstCaseMiB(rows) {
  const numeric = Number(rows);
  if (!Number.isFinite(numeric) || numeric <= 0) return 0;
  const storedRows = Math.floor(numeric);
  const ringSlots = 2 ** Math.ceil(Math.log2(storedRows));
  const rowBytes = (storedRows + SCROLLBACK_SCREEN_ROWS_AT_MEASUREMENT)
    * SCROLLBACK_ROW_BYTES_AT_95_COLUMNS;
  return Math.round(
    (rowBytes + ringSlots * SCROLLBACK_RING_BYTES_PER_SLOT) / BYTES_PER_MEBIBYTE,
  );
}

function scrollbackRowsLabel(rows) {
  return rows % 1000 === 0 ? `${rows / 1000}k` : String(rows);
}

function scrollbackRowsAriaLabel(rows) {
  return t("settings.terminal.scrollbackCost", "{{rows}}줄 · 판당 ≤{{mib}}MiB", {
    rows: Number(rows).toLocaleString(document.documentElement.lang),
    mib: scrollbackWorstCaseMiB(rows),
  });
}

function scrollbackCostTitle(rows) {
  return t(
    "settings.terminal.scrollbackCostBasis",
    "95칸 기준 최악 · 판당 ≤{{mib}}MiB",
    { mib: scrollbackWorstCaseMiB(rows) },
  );
}

function paintScrollbackCostCaption(rows) {
  const group = el("term-scrollback-presets");
  let caption = document.getElementById("term-scrollback-cost");
  if (!caption) {
    caption = document.createElement("p");
    caption.id = "term-scrollback-cost";
    caption.className = "settings-hint";
    caption.setAttribute("role", "status");
    caption.setAttribute("aria-live", "polite");
    group.insertAdjacentElement("afterend", caption);
  }
  const asked = Number(rows);
  const numeric = Number.isFinite(asked) && asked > 0 ? asked : termScrollbackCanonical;
  caption.textContent = t(
    "settings.terminal.scrollbackCostCaption",
    "{{rows}}줄 · 판당 ≤{{mib}}MiB — 95칸 최악",
    {
      rows: Math.floor(numeric).toLocaleString(document.documentElement.lang),
      mib: scrollbackWorstCaseMiB(numeric),
    },
  );
}

/* The preset list is renderer data, but not renderer policy: Rust supplies
 * every value and bound. A local draft exists only so an unrelated settings
 * repaint cannot erase a number while it is being typed. */
function paintScrollbackSetting() {
  const spec = termPrefsSpec?.scrollback;
  const presets = termPrefsSpec?.scrollback_presets ?? [];
  const canonical = Number(termPrefs.scrollback);
  if (termScrollbackCanonical !== canonical) {
    termScrollbackCanonical = canonical;
    termScrollbackDraft = String(canonical);
    termScrollbackCustomRequested = !presets.includes(canonical);
  }

  const group = el("term-scrollback-presets");
  const signature = presets.join(",");
  if (group.dataset.presetSignature !== signature) {
    const options = document.createDocumentFragment();
    for (const preset of presets) {
      const button = document.createElement("button");
      button.type = "button";
      button.className = "segment-btn";
      button.dataset.scrollbackRows = String(preset);
      options.appendChild(button);
    }
    const custom = document.createElement("button");
    custom.type = "button";
    custom.className = "segment-btn";
    custom.dataset.scrollbackCustom = "";
    options.appendChild(custom);
    group.replaceChildren(options);
    group.dataset.presetSignature = signature;
  }

  const customMode = termScrollbackCustomRequested || !presets.includes(canonical);
  for (const button of group.querySelectorAll("[data-scrollback-rows]")) {
    const rows = Number(button.dataset.scrollbackRows);
    const active = !customMode && rows === canonical;
    button.classList.toggle("is-active", active);
    button.setAttribute("aria-pressed", String(active));
    button.textContent = scrollbackRowsLabel(rows);
    button.setAttribute("aria-label", scrollbackRowsAriaLabel(rows));
    button.dataset.tip = scrollbackCostTitle(rows);
  }
  const customButton = group.querySelector("[data-scrollback-custom]");
  customButton.textContent = t("settings.terminal.scrollbackCustom", "사용자 지정");
  customButton.classList.toggle("is-active", customMode);
  customButton.setAttribute("aria-pressed", String(customMode));

  const field = el("term-scrollback");
  if (spec) {
    field.min = String(spec.min);
    field.max = String(spec.max);
    field.step = String(spec.step);
  }
  field.value = termScrollbackDraft;
  paintScrollbackCostCaption(customMode ? termScrollbackDraft : canonical);
  el("term-scrollback-custom").hidden = !customMode;
}

const TERMINAL_COLOR_LABELS = Object.freeze({
  foreground: { key: "settings.terminal.colorForeground", name: "전경" },
  background: { key: "settings.terminal.colorBackground", name: "배경" },
  cursor: { key: "settings.terminal.colorCursor", name: "커서" },
  cursorAccent: { key: "settings.terminal.colorCursorAccent", name: "커서 안 글자" },
  selectionBackground: { key: "settings.terminal.colorSelectionBackground", name: "선택 배경" },
  selectionForeground: { key: "settings.terminal.colorSelectionForeground", name: "선택 전경" },
  bold: { key: "settings.terminal.colorBold", name: "굵은 글자" },
  black: { key: "settings.terminal.colorBlack", name: "검정" },
  red: { key: "settings.terminal.colorRed", name: "빨강" },
  green: { key: "settings.terminal.colorGreen", name: "초록" },
  yellow: { key: "settings.terminal.colorYellow", name: "노랑" },
  blue: { key: "settings.terminal.colorBlue", name: "파랑" },
  magenta: { key: "settings.terminal.colorMagenta", name: "자홍" },
  cyan: { key: "settings.terminal.colorCyan", name: "청록" },
  white: { key: "settings.terminal.colorWhite", name: "흰색" },
  brightBlack: { key: "settings.terminal.colorBrightBlack", name: "밝은 검정" },
  brightRed: { key: "settings.terminal.colorBrightRed", name: "밝은 빨강" },
  brightGreen: { key: "settings.terminal.colorBrightGreen", name: "밝은 초록" },
  brightYellow: { key: "settings.terminal.colorBrightYellow", name: "밝은 노랑" },
  brightBlue: { key: "settings.terminal.colorBrightBlue", name: "밝은 파랑" },
  brightMagenta: { key: "settings.terminal.colorBrightMagenta", name: "밝은 자홍" },
  brightCyan: { key: "settings.terminal.colorBrightCyan", name: "밝은 청록" },
  brightWhite: { key: "settings.terminal.colorBrightWhite", name: "밝은 흰색" },
});

function terminalColorLabel(key) {
  const label = TERMINAL_COLOR_LABELS[key] ?? { key, name: key };
  return t(label.key, label.name);
}

function terminalColorDescription(key) {
  if (key !== "bold") return "";
  return t(
    "settings.terminal.colorBoldHint",
    "설정에는 보존되지만 터미널 renderer에 굵은 글자 전용 색상 슬롯이 없어 아직 적용되지 않습니다.",
  );
}

function terminalColorGroupLabel(id) {
  if (id === "normal") return t("settings.terminal.colorNormal", "ANSI 기본색");
  if (id === "bright") return t("settings.terminal.colorBright", "ANSI 밝은색");
  return t("settings.terminal.colorBase", "기본");
}

function terminalThemeModeLabel(mode) {
  if (mode === "dark") return t("settings.terminal.themeModeDark", "다크");
  if (mode === "light") return t("settings.terminal.themeModeLight", "라이트");
  return t("settings.terminal.themeModeUnknown", "지정 안 함");
}

function terminalThemeMeta(theme) {
  return [theme.sourceLabel, "Warp", terminalThemeModeLabel(theme.mode)]
    .filter(Boolean)
    .join(" · ");
}

function terminalThemeSwatches(theme) {
  const swatches = document.createElement("span");
  swatches.className = "terminal-theme-swatches";
  for (const key of ["background", "foreground", "red", "blue"]) {
    const color = document.createElement("i");
    color.style.backgroundColor = theme.terminal?.[key] ?? "transparent";
    swatches.appendChild(color);
  }
  return swatches;
}

function paintTerminalThemeSelect(select, chosen) {
  select.replaceChildren();
  for (const name of Object.keys(termPrefsSpec?.terminal_themes ?? {})) {
    const option = document.createElement("option");
    option.value = name;
    option.textContent = name;
    option.selected = name === chosen;
    select.appendChild(option);
  }
  const themes = [...(termPrefs?.custom_themes ?? [])]
    .sort((left, right) => left.name.localeCompare(right.name, locale));
  for (const theme of themes) {
    const value = `${termPrefsSpec.custom_theme_selection_prefix}${theme.id}`;
    const option = document.createElement("option");
    option.value = value;
    option.textContent = `${theme.name} · Warp`;
    option.selected = value === chosen;
    select.appendChild(option);
  }
}

function paintTerminalCustomThemes() {
  const themes = termPrefs?.custom_themes ?? [];
  const status = el("term-theme-import-status");
  status.textContent = terminalThemeImportBusy
    ? t("settings.terminal.importScanning", "Warp 테마를 확인하고 있습니다…")
    : themes.length === 0
      ? t("settings.terminal.importNone", "아직 가져온 테마가 없습니다.")
      : t("settings.terminal.importInstalled", "가져온 테마 {{count}}개", {
        count: themes.length,
      });
  const list = el("term-custom-theme-list");
  list.replaceChildren();
  for (const theme of themes) {
    const row = document.createElement("div");
    row.className = "terminal-custom-theme-row";
    row.appendChild(terminalThemeSwatches(theme));
    const copy = document.createElement("span");
    copy.className = "terminal-custom-theme-copy";
    const name = document.createElement("span");
    name.className = "terminal-custom-theme-name";
    name.textContent = theme.name;
    const meta = document.createElement("span");
    meta.className = "terminal-custom-theme-meta";
    meta.textContent = terminalThemeMeta(theme);
    copy.append(name, meta);
    const remove = document.createElement("button");
    remove.className = "btn btn--halt-quiet";
    remove.type = "button";
    remove.dataset.removeCustomTheme = theme.id;
    remove.textContent = t("settings.terminal.importRemove", "테마 제거");
    remove.addEventListener("click", () => removeCustomTerminalTheme(theme.id));
    row.append(copy, remove);
    list.appendChild(row);
  }
}

function paintTerminalThemeControls() {
  paintTerminalThemeSelect(el("term-theme-dark"), termPrefs.theme_dark);
  paintTerminalThemeSelect(el("term-theme-light"), termPrefs.theme_light);
  el("term-separate-light-theme").checked = termPrefs.use_separate_light_theme === true;
  el("term-theme-light-field").hidden = termPrefs.use_separate_light_theme !== true;
  paintTerminalCustomThemes();
}

function buildTerminalColorOverrides() {
  const host = el("term-color-override-groups");
  const groups = termPrefsSpec?.color_override_groups ?? [];
  const signature = `${locale}\u0000${JSON.stringify(groups)}`;
  if (host.dataset.signature === signature) return;
  host.dataset.signature = signature;
  host.replaceChildren();
  for (const group of groups) {
    const section = document.createElement("section");
    section.className = "terminal-color-group";
    const heading = document.createElement("h4");
    heading.className = "terminal-color-group-title";
    heading.textContent = terminalColorGroupLabel(group.id);
    const fields = document.createElement("div");
    fields.className = "terminal-color-grid";
    for (const key of group.keys) {
      const id = `term-color-${key}`;
      const field = document.createElement("label");
      field.className = "settings-field terminal-color-field";
      field.htmlFor = id;
      const label = document.createElement("span");
      label.className = "settings-label";
      label.textContent = terminalColorLabel(key);
      const description = terminalColorDescription(key);
      const hint = description === "" ? null : document.createElement("span");
      if (hint) {
        hint.className = "settings-hint";
        hint.textContent = description;
      }
      const controls = document.createElement("span");
      controls.className = "settings-color-control";
      const swatch = document.createElement("input");
      swatch.className = "settings-color-swatch";
      swatch.type = "color";
      swatch.dataset.terminalColorSwatch = key;
      swatch.setAttribute("aria-label", terminalColorLabel(key));
      const text = document.createElement("input");
      text.className = "settings-input settings-input--mono";
      text.id = id;
      text.type = "text";
      text.spellcheck = false;
      text.autocomplete = "off";
      text.dataset.terminalColorText = key;
      swatch.addEventListener("input", (event) => {
        setTermColorOverride(key, event.target.value);
      });
      text.addEventListener("change", (event) => {
        const raw = event.target.value.trim();
        const color = raw === "" ? "" : normalizedHexColor(raw, "");
        if (raw !== "" && color === "") {
          paintTerminalColorOverrides();
          return;
        }
        setTermColorOverride(key, color);
      });
      controls.append(swatch, text);
      field.append(label);
      if (hint) field.append(hint);
      field.append(controls);
      fields.appendChild(field);
    }
    section.append(heading, fields);
    host.appendChild(section);
  }
}

function paintTerminalColorOverrides() {
  buildTerminalColorOverrides();
  const palette = terminalThemePalette() ?? {};
  const overrides = termPrefs.color_overrides ?? {};
  for (const key of Object.keys(TERMINAL_COLOR_LABELS)) {
    const effective = overrides[key] ?? palette[key] ?? "#000000";
    const text = document.querySelector(`[data-terminal-color-text="${key}"]`);
    const swatch = document.querySelector(`[data-terminal-color-swatch="${key}"]`);
    if (text) {
      text.value = overrides[key] ?? "";
      text.placeholder = palette[key] ?? "";
    }
    if (swatch) swatch.value = effective;
  }
  el("term-reset-color-overrides").disabled = Object.keys(overrides).length === 0;
}

function paintTermPrefs() {
  if (termPrefs === null) return;
  paintTerminalThemeControls();
  paintTerminalColorOverrides();
  paintNumberSetting("term-font-size", termPrefsSpec?.font_size, termPrefs.font_size);
  el("term-font-family").value = terminalFontFamilyChoice(
    termPrefs.font_family,
    termPrefsSpec,
  );
  optionRow(
    el("term-ligatures"),
    termPrefsSpec?.ligature_modes ?? [termPrefs.ligatures],
    termPrefs.ligatures,
    terminalLigatureLabel,
  );
  el("term-ligatures-status").textContent = terminalLigatureStatus(
    termPrefs.ligatures,
    termPrefs.font_family,
    termPrefsSpec,
  );
  const optionField = el("term-option-as-alt-field");
  optionField.hidden = !usesCommandModifier;
  if (usesCommandModifier) {
    optionRow(
      el("term-option-as-alt"),
      termPrefsSpec?.mac_option_as_alt_modes ?? [termPrefs.mac_option_as_alt],
      termPrefs.mac_option_as_alt,
      macOptionAsAltLabel,
    );
    el("term-option-as-alt-status").textContent = macOptionAsAltStatus();
  }
  el("term-jis-yen-to-backslash-field").hidden = !usesCommandModifier;
  el("term-jis-yen-to-backslash").checked = termPrefs.jis_yen_to_backslash === true;
  paintNumberSetting(
    "term-leading",
    termPrefsSpec?.leading,
    terminalLineHeightControlValue(termPrefs.leading, termPrefsSpec),
  );
  paintNumberSetting("term-weight", termPrefsSpec?.weight, termPrefs.weight);
  optionRow(
    el("term-cursor-style"),
    termPrefsSpec?.cursor_styles ?? [termPrefs.cursor_style],
    termPrefs.cursor_style,
    cursorStyleLabel,
  );
  paintNumberSetting(
    "term-cursor-opacity",
    termPrefsSpec?.cursor_opacity,
    termPrefs.cursor_opacity,
  );
  paintNumberSetting("term-padding-x", termPrefsSpec?.padding_x, termPrefs.padding_x);
  paintNumberSetting("term-padding-y", termPrefsSpec?.padding_y, termPrefs.padding_y);
  paintNumberSetting(
    "term-inactive-pane-opacity",
    termPrefsSpec?.inactive_pane_opacity,
    termPrefs.inactive_pane_opacity,
  );
  paintNumberSetting(
    "term-divider-thickness",
    termPrefsSpec?.divider_thickness_px,
    termPrefs.divider_thickness_px,
  );
  for (const kind of ["divider_color_dark", "divider_color_light"]) {
    const id = `term-${kind.replaceAll("_", "-")}`;
    el(id).value = termPrefs[kind];
    el(`${id}-swatch`).value = termPrefs[kind];
  }
  paintNumberSetting("term-sensitivity", termPrefsSpec?.sensitivity, termPrefs.sensitivity);
  paintScrollbackSetting();
  paintNumberSetting(
    "term-fast-scroll",
    termPrefsSpec?.fast_scroll_sensitivity,
    termPrefs.fast_scroll_sensitivity,
  );
  paintNumberSetting(
    "term-tui-scroll",
    termPrefsSpec?.tui_scroll_sensitivity,
    termPrefs.tui_scroll_sensitivity,
  );
  const separators = el("term-word-separators");
  separators.value = termPrefs.word_separators;
  if (termPrefsSpec?.word_separators_max_chars) {
    separators.maxLength = termPrefsSpec.word_separators_max_chars;
  }
  const windowsShellField = el("term-windows-shell-field");
  windowsShellField.hidden = !usesWindowsPlatform;
  const selectedWindowsShell = termPrefs.windows_shell ?? "powershell.exe";
  if (usesWindowsPlatform) {
    const shellSelect = el("term-windows-shell");
    const shellModes = termPrefsSpec?.windows_shells ?? [selectedWindowsShell];
    optionRow(
      shellSelect,
      shellModes.filter((mode) => (
        mode !== "git-bash"
        || terminalWindowsStatus?.git_bash_available === true
        || selectedWindowsShell === "git-bash"
      )),
      selectedWindowsShell,
      windowsTerminalShellLabel,
    );
    const gitBashOption = shellSelect.querySelector('option[value="git-bash"]');
    if (gitBashOption) gitBashOption.disabled = terminalWindowsStatus?.git_bash_available !== true;
    el("term-windows-shell-status").textContent = windowsTerminalShellStatusText();
  }
  const powershellField = el("term-windows-powershell-field");
  powershellField.hidden = !usesWindowsPlatform || selectedWindowsShell !== "powershell.exe";
  if (!powershellField.hidden) {
    const select = el("term-windows-powershell");
    optionRow(
      select,
      termPrefsSpec?.windows_powershell_implementations
        ?? [termPrefs.windows_powershell_implementation],
      termPrefs.windows_powershell_implementation,
      windowsPowerShellImplementationLabel,
    );
    const pwshOption = select.querySelector('option[value="pwsh.exe"]');
    if (pwshOption) pwshOption.disabled = terminalWindowsStatus?.pwsh_available !== true;
    el("term-windows-powershell-status").textContent = windowsPowerShellStatusText();
  }
  el("term-cursor-blink").checked = termPrefs.cursor_blink === true;
  el("term-focus-follows-mouse").checked = termPrefs.focus_follows_mouse === true;
  el("term-hide-mouse-while-typing").checked = termPrefs.hide_mouse_while_typing === true;
  el("term-copy-on-select").checked = termPrefs.copy_on_select === true;
  el("term-right-click-paste").checked = termPrefs.right_click_paste === true;
  el("term-allow-osc52-clipboard").checked = termPrefs.allow_osc52_clipboard === true;
}

function setTermPrefs(change) {
  const before = termPrefs;
  termPrefs = { ...(termPrefs ?? {}), ...change };
  applyTermPrefs(before);
  paintTermPrefs();
  const [kind, value] = Object.entries(change)[0];
  void commitSetting(`terminal_prefs.${kind}`, "patch_terminal_prefs", {
    patch: { kind, value },
  });
}

function setTermColorOverride(key, color) {
  const before = termPrefs;
  const overrides = { ...(termPrefs?.color_overrides ?? {}) };
  if (color) overrides[key] = color;
  else delete overrides[key];
  termPrefs = { ...termPrefs, color_overrides: overrides };
  applyTermPrefs(before);
  paintTermPrefs();
  void commitSetting(`terminal_prefs.color_overrides.${key}`, "patch_terminal_prefs", {
    patch: {
      kind: "color_override",
      value: { key, value: color || null },
    },
  });
}

function resetTermColorOverrides() {
  const before = termPrefs;
  termPrefs = { ...termPrefs, color_overrides: {} };
  applyTermPrefs(before);
  paintTermPrefs();
  void commitSetting("terminal_prefs.color_overrides", "patch_terminal_prefs", {
    patch: { kind: "reset_color_overrides" },
  });
}

function mergedCustomTerminalThemes(existing, incoming) {
  const limit = termPrefsSpec?.max_custom_terminal_themes ?? 0;
  if (!Number.isSafeInteger(limit) || limit <= 0) return [];
  const combined = [...existing, ...incoming];
  const seen = new Set();
  const merged = [];
  for (let index = combined.length - 1; index >= 0 && merged.length < limit; index -= 1) {
    const theme = combined[index];
    if (!seen.has(theme.id)) {
      seen.add(theme.id);
      merged.push(theme);
    }
  }
  return merged.reverse();
}

function removeCustomTerminalTheme(id) {
  const removedSelection = `${termPrefsSpec.custom_theme_selection_prefix}${id}`;
  const before = termPrefs;
  const next = {
    ...termPrefs,
    custom_themes: (termPrefs.custom_themes ?? []).filter((theme) => theme.id !== id),
  };
  if (next.theme_dark === removedSelection) {
    next.theme_dark = termPrefsSpec.theme_defaults.dark;
  }
  if (next.theme_light === removedSelection) {
    next.theme_light = termPrefsSpec.theme_defaults.light;
  }
  termPrefs = next;
  applyTermPrefs(before);
  paintTermPrefs();
  void commitSetting("terminal_prefs.custom_themes", "patch_terminal_prefs", {
    patch: { kind: "remove_custom_theme", value: id },
  });
}

function closeTerminalThemeImport() {
  hideModal(el("term-theme-import-scrim"));
  terminalThemeImportPreview = null;
  el("term-theme-preview-list").replaceChildren();
  el("term-theme-preview-skipped").textContent = "";
  terminalThemeImportReturnFocus = null;
}

function paintTerminalThemeImportPreview() {
  const preview = terminalThemeImportPreview ?? { themes: [], skipped: [] };
  const list = el("term-theme-preview-list");
  list.replaceChildren();
  for (const theme of preview.themes ?? []) {
    const row = document.createElement("label");
    row.className = "terminal-theme-preview-row";
    const checkbox = document.createElement("input");
    checkbox.type = "checkbox";
    checkbox.checked = true;
    checkbox.dataset.importThemeId = theme.id;
    checkbox.addEventListener("change", paintTerminalThemeImportSelection);
    row.appendChild(checkbox);
    row.appendChild(terminalThemeSwatches(theme));
    const copy = document.createElement("span");
    copy.className = "terminal-theme-preview-copy";
    const name = document.createElement("span");
    name.className = "terminal-theme-preview-name";
    name.textContent = theme.name;
    const meta = document.createElement("span");
    meta.className = "terminal-theme-preview-meta";
    const details = [terminalThemeMeta(theme)];
    if ((theme.unsupportedFeatures ?? []).length > 0) {
      details.push(t("settings.terminal.importUnsupported", "지원하지 않는 효과 제외"));
    }
    meta.textContent = details.filter(Boolean).join(" · ");
    copy.append(name, meta);
    row.appendChild(copy);
    list.appendChild(row);
  }
  if ((preview.themes ?? []).length === 0) {
    const empty = document.createElement("p");
    empty.className = "settings-hint";
    empty.textContent = t(
      "settings.terminal.importEmpty",
      "호환되는 Warp 테마를 찾지 못했습니다.",
    );
    list.appendChild(empty);
  }
  const skipped = preview.skipped?.length ?? 0;
  el("term-theme-preview-skipped").textContent = skipped === 0
    ? ""
    : t(
      "settings.terminal.importSkipped",
      "유효하지 않거나 지원하지 않거나 안전 제한을 넘은 파일 {{count}}개를 건너뛰었습니다.",
      { count: skipped },
    );
  paintTerminalThemeImportSelection();
}

function selectedTerminalThemeImports() {
  const selected = new Set(
    [...document.querySelectorAll("[data-import-theme-id]:checked")]
      .map((checkbox) => checkbox.dataset.importThemeId),
  );
  return (terminalThemeImportPreview?.themes ?? [])
    .filter((theme) => selected.has(theme.id));
}

function paintTerminalThemeImportSelection() {
  el("term-theme-import-apply").disabled = selectedTerminalThemeImports().length === 0;
}

async function openTerminalThemeImport(source) {
  if (terminalThemeImportBusy) return;
  terminalThemeImportBusy = true;
  terminalThemeImportReturnFocus = document.activeElement;
  paintTerminalCustomThemes();
  for (const button of document.querySelectorAll(".terminal-theme-import-actions button")) {
    button.disabled = true;
  }
  try {
    const preview = await invoke("preview_warp_terminal_themes", { source });
    if (preview === null) {
      terminalThemeImportReturnFocus = null;
      return;
    }
    terminalThemeImportPreview = preview;
    paintTerminalThemeImportPreview();
    showModal(el("term-theme-import-scrim"), { opener: terminalThemeImportReturnFocus });
  } catch (error) {
    terminalThemeImportReturnFocus = null;
    showError(error);
  } finally {
    terminalThemeImportBusy = false;
    for (const button of document.querySelectorAll(".terminal-theme-import-actions button")) {
      button.disabled = false;
    }
    paintTerminalCustomThemes();
  }
}

function applyTerminalThemeImports() {
  const selected = selectedTerminalThemeImports();
  if (selected.length === 0) return;
  const before = termPrefs;
  termPrefs = {
    ...termPrefs,
    custom_themes: mergedCustomTerminalThemes(termPrefs.custom_themes ?? [], selected),
  };
  applyTermPrefs(before);
  paintTermPrefs();
  closeTerminalThemeImport();
  void commitSetting("terminal_prefs.custom_themes", "patch_terminal_prefs", {
    patch: { kind: "import_custom_themes", value: selected },
  });
}

const GHOSTTY_CHANGE_LABELS = Object.freeze({
  terminalMacOptionAsAlt: "settings.terminal.optionAsAlt",
  terminalBackgroundOpacity: "settings.window.opacity",
  windowBackgroundBlur: "settings.window.blur",
  terminalColorOverrides: "settings.terminal.colorOverrides",
  terminalDividerColorDark: "settings.terminal.dividerColorDark",
  terminalDividerColorLight: "settings.terminal.dividerColorLight",
  terminalInactivePaneOpacity: "settings.terminal.inactivePaneOpacity",
  terminalPaddingX: "settings.terminal.paddingX",
  terminalPaddingY: "settings.terminal.paddingY",
  terminalLineHeight: "settings.terminal.leading",
  terminalMouseHideWhileTyping: "settings.terminal.hideMouseWhileTyping",
  terminalCursorOpacity: "settings.terminal.cursorOpacity",
  terminalFontFamily: "settings.terminal.fontFamily",
  terminalFontSize: "settings.terminal.fontSize",
  terminalFontWeight: "settings.terminal.weight",
  terminalCursorStyle: "settings.terminal.cursorStyle",
  terminalCursorBlink: "settings.terminal.cursorBlink",
  terminalFocusFollowsMouse: "settings.terminal.focusFollowsMouse",
  primarySelectionMiddleClickPaste: "settings.editing.primarySelectionMiddleClickPaste",
});

function closeGhosttyImport() {
  const scrim = el("term-ghostty-import-scrim");
  hideModal(scrim);
  ghosttyImportRequest += 1;
  ghosttyImportPreview = null;
  ghosttyImportBusy = false;
  ghosttyImportApplied = false;
  ghosttyImportApplyError = "";
  ghosttyImportReturnFocus = null;
}

function paintGhosttyImportPreview() {
  const preview = ghosttyImportPreview;
  const paths = preview?.configPaths ?? [];
  const changes = preview?.changes ?? [];
  const unsupported = preview?.unsupportedKeys ?? [];
  const message = el("term-ghostty-import-message");
  const pathNote = el("term-ghostty-import-paths");
  const list = el("term-ghostty-import-changes");
  const unsupportedBox = el("term-ghostty-import-unsupported");
  const unsupportedList = el("term-ghostty-import-unsupported-list");
  const error = el("term-ghostty-import-error");
  const cancel = el("term-ghostty-import-cancel");
  const apply = el("term-ghostty-import-apply");

  pathNote.textContent = paths.length === 0 || ghosttyImportApplied
    ? ""
    : `${t(
      paths.length === 1
        ? "settings.terminal.ghosttyConfig"
        : "settings.terminal.ghosttyConfigs",
      paths.length === 1 ? "설정" : "설정들",
    )}: ${paths.join(", ")}`;
  message.textContent = ghosttyImportBusy
    ? t("settings.terminal.ghosttyLoading", "미리보기를 불러오는 중…")
    : ghosttyImportApplied
      ? t("settings.terminal.ghosttyComplete", "가져오기 완료")
      : preview?.found === false && !preview?.error
        ? t("settings.terminal.ghosttyNotFound", "이 시스템에서 Ghostty 설정을 찾지 못했습니다.")
        : preview?.found === true && changes.length === 0
          ? t(
            "settings.terminal.ghosttyNoChanges",
            "가져올 새 설정이 없습니다. 현재 설정과 이미 같습니다.",
          )
          : changes.length > 0
            ? t("settings.terminal.ghosttyChanges", "업데이트할 설정")
            : "";

  list.replaceChildren();
  for (const change of changes) {
    const row = document.createElement("div");
    row.className = "terminal-theme-preview-row terminal-ghostty-change";
    const label = document.createElement("span");
    label.textContent = t(GHOSTTY_CHANGE_LABELS[change.key] ?? change.key, change.key);
    const value = document.createElement("span");
    value.textContent = change.value;
    row.append(label, value);
    list.appendChild(row);
  }

  unsupportedBox.hidden = ghosttyImportApplied || unsupported.length === 0;
  unsupportedList.replaceChildren();
  for (const key of unsupported) {
    const item = document.createElement("li");
    item.textContent = key;
    unsupportedList.appendChild(item);
  }

  const errorText = ghosttyImportApplyError || preview?.error || "";
  error.textContent = errorText;
  error.hidden = errorText.length === 0;
  cancel.hidden = ghosttyImportApplied;
  apply.textContent = ghosttyImportApplied
    ? t("app.close", "닫기")
    : t("settings.terminal.ghosttyApply", "변경 적용");
  apply.disabled = ghosttyImportBusy || (!ghosttyImportApplied && changes.length === 0);
}

async function openGhosttyImport() {
  if (ghosttyImportBusy) return;
  ghosttyImportReturnFocus = document.activeElement;
  ghosttyImportPreview = null;
  ghosttyImportApplied = false;
  ghosttyImportApplyError = "";
  ghosttyImportBusy = true;
  const request = ++ghosttyImportRequest;
  paintGhosttyImportPreview();
  showModal(el("term-ghostty-import-scrim"), { opener: ghosttyImportReturnFocus });
  try {
    const preview = await invoke("preview_ghostty_import");
    if (request !== ghosttyImportRequest) return;
    ghosttyImportPreview = preview;
  } catch (error) {
    if (request !== ghosttyImportRequest) return;
    ghosttyImportPreview = {
      found: false,
      configPaths: [],
      patch: {},
      changes: [],
      unsupportedKeys: [],
      error: String(error),
    };
  } finally {
    if (request === ghosttyImportRequest) {
      ghosttyImportBusy = false;
      paintGhosttyImportPreview();
    }
  }
}

async function applyGhosttyImport() {
  if (ghosttyImportApplied) {
    closeGhosttyImport();
    return;
  }
  if (ghosttyImportBusy || (ghosttyImportPreview?.changes?.length ?? 0) === 0) return;
  ghosttyImportBusy = true;
  ghosttyImportApplyError = "";
  paintGhosttyImportPreview();
  const snapshot = await commitSetting(
    "ghostty_import",
    "apply_ghostty_import",
    { patch: ghosttyImportPreview.patch },
    { onError: (error) => { ghosttyImportApplyError = String(error); } },
  );
  ghosttyImportBusy = false;
  if (snapshot) ghosttyImportApplied = true;
  paintGhosttyImportPreview();
}

el("term-font-size").addEventListener("change", (event) => {
  setTermPrefs({ font_size: Number(event.target.value) });
});
el("term-font-family").addEventListener("change", (event) => {
  const family = event.target.value.trim();
  const canonical = family === terminalDefaultFontFamily(termPrefsSpec) ? "" : family;
  if (canonical === termPrefs.font_family) {
    paintTermPrefs();
    return;
  }
  setTermPrefs({ font_family: canonical });
});
el("term-ligatures").addEventListener("change", (event) => {
  setTermPrefs({ ligatures: event.target.value });
});
el("term-option-as-alt").addEventListener("change", (event) => {
  setTermPrefs({ mac_option_as_alt: event.target.value });
});
el("term-jis-yen-to-backslash").addEventListener("change", (event) => {
  setTermPrefs({ jis_yen_to_backslash: event.target.checked });
});
el("term-theme-dark").addEventListener("change", (event) => {
  setTermPrefs({ theme_dark: event.target.value });
});
el("term-separate-light-theme").addEventListener("change", (event) => {
  setTermPrefs({ use_separate_light_theme: event.target.checked });
});
el("term-theme-light").addEventListener("change", (event) => {
  setTermPrefs({ theme_light: event.target.value });
});
el("term-theme-import-auto").addEventListener("click", () => {
  void openTerminalThemeImport("auto");
});
el("term-theme-import-files").addEventListener("click", () => {
  void openTerminalThemeImport("files");
});
el("term-theme-import-folder").addEventListener("click", () => {
  void openTerminalThemeImport("folder");
});
el("term-theme-import-cancel").addEventListener("click", closeTerminalThemeImport);
el("term-theme-import-apply").addEventListener("click", applyTerminalThemeImports);
el("term-theme-import-scrim").addEventListener("click", (event) => {
  if (event.target === event.currentTarget) closeTerminalThemeImport();
});
el("term-ghostty-import").addEventListener("click", () => {
  void openGhosttyImport();
});
el("term-ghostty-import-cancel").addEventListener("click", closeGhosttyImport);
el("term-ghostty-import-apply").addEventListener("click", () => {
  void applyGhosttyImport();
});
el("term-ghostty-import-scrim").addEventListener("click", (event) => {
  if (event.target === event.currentTarget) closeGhosttyImport();
});
el("term-reset-color-overrides").addEventListener("click", resetTermColorOverrides);
el("term-leading").addEventListener("change", (event) => {
  setTermPrefs({
    leading: terminalLineHeightStorageValue(Number(event.target.value), termPrefsSpec),
  });
});
el("term-weight").addEventListener("change", (event) => {
  setTermPrefs({ weight: Number(event.target.value) });
});
el("term-cursor-blink").addEventListener("change", (event) => {
  setTermPrefs({ cursor_blink: event.target.checked });
});
el("term-cursor-style").addEventListener("change", (event) => {
  setTermPrefs({ cursor_style: event.target.value });
});
el("term-cursor-opacity").addEventListener("change", (event) => {
  setTermPrefs({ cursor_opacity: Number(event.target.value) });
});
el("term-padding-x").addEventListener("change", (event) => {
  setTermPrefs({ padding_x: Number(event.target.value) });
});
el("term-padding-y").addEventListener("change", (event) => {
  setTermPrefs({ padding_y: Number(event.target.value) });
});
el("term-inactive-pane-opacity").addEventListener("change", (event) => {
  setTermPrefs({ inactive_pane_opacity: Number(event.target.value) });
});
el("term-divider-thickness").addEventListener("change", (event) => {
  setTermPrefs({ divider_thickness_px: Number(event.target.value) });
});
for (const kind of ["divider_color_dark", "divider_color_light"]) {
  const id = `term-${kind.replaceAll("_", "-")}`;
  for (const control of [el(id), el(`${id}-swatch`)]) {
    control.addEventListener(control.type === "color" ? "input" : "change", (event) => {
      setTermPrefs({
        [kind]: normalizedTerminalDividerColor(kind, event.target.value, termPrefsSpec),
      });
    });
  }
}
el("term-sensitivity").addEventListener("change", (event) => {
  setTermPrefs({ sensitivity: Number(event.target.value) });
});
el("term-scrollback-presets").addEventListener("click", (event) => {
  const button = event.target.closest?.(".segment-btn");
  if (!button || !event.currentTarget.contains(button)) return;
  if (button.hasAttribute("data-scrollback-custom")) {
    termScrollbackCustomRequested = true;
    paintScrollbackSetting();
    el("term-scrollback").focus();
    el("term-scrollback").select();
    return;
  }
  termScrollbackCustomRequested = false;
  setTermPrefs({ scrollback: Number(button.dataset.scrollbackRows) });
});
el("term-scrollback").addEventListener("input", (event) => {
  termScrollbackDraft = event.target.value;
  paintScrollbackCostCaption(termScrollbackDraft);
});

function commitScrollbackDraft() {
  const field = el("term-scrollback");
  const trimmed = field.value.trim();
  const value = Number(trimmed);
  if (trimmed === "" || !Number.isFinite(value)) {
    termScrollbackDraft = String(termScrollbackCanonical);
    field.value = termScrollbackDraft;
    paintScrollbackCostCaption(termScrollbackDraft);
    return;
  }
  const spec = termPrefsSpec?.scrollback;
  const floored = Math.floor(value);
  const next = spec ? Math.min(spec.max, Math.max(spec.min, floored)) : floored;
  termScrollbackDraft = String(next);
  field.value = termScrollbackDraft;
  paintScrollbackCostCaption(termScrollbackDraft);
  if (next !== termScrollbackCanonical) setTermPrefs({ scrollback: next });
}

el("term-scrollback").addEventListener("blur", commitScrollbackDraft);
el("term-scrollback").addEventListener("keydown", (event) => {
  if (event.key !== "Enter") return;
  event.preventDefault();
  commitScrollbackDraft();
});
el("term-word-separators").addEventListener("change", (event) => {
  setTermPrefs({ word_separators: event.target.value });
});
el("term-windows-shell").addEventListener("change", (event) => {
  setTermPrefs({ windows_shell: event.target.value });
});
el("term-windows-powershell").addEventListener("change", (event) => {
  setTermPrefs({ windows_powershell_implementation: event.target.value });
});
el("term-fast-scroll").addEventListener("change", (event) => {
  setTermPrefs({ fast_scroll_sensitivity: Number(event.target.value) });
});
el("term-tui-scroll").addEventListener("change", (event) => {
  setTermPrefs({ tui_scroll_sensitivity: Number(event.target.value) });
});
el("term-focus-follows-mouse").addEventListener("change", (event) => {
  setTermPrefs({ focus_follows_mouse: event.target.checked });
});
el("term-hide-mouse-while-typing").addEventListener("change", (event) => {
  setTermPrefs({ hide_mouse_while_typing: event.target.checked });
});
el("term-copy-on-select").addEventListener("change", (event) => {
  setTermPrefs({ copy_on_select: event.target.checked });
});
el("term-right-click-paste").addEventListener("change", (event) => {
  setTermPrefs({ right_click_paste: event.target.checked });
});
el("term-allow-osc52-clipboard").addEventListener("change", (event) => {
  setTermPrefs({ allow_osc52_clipboard: event.target.checked });
});

/* What a new terminal runs. Empty means the shell you already use, which is
 * what it does unless you say otherwise — the centre is a terminal, and what
 * runs in it is your call. */
const terminalCommandField = el("terminal-command");
let terminalCommand = "";

/* The line comes back already quoted. Rust owns both halves of that round
 * trip — the window quoting it a second time is how the two rules drifted
 * apart and started rewriting argv on save. */
function loadTerminalCommand() {
  return refreshSettingsSnapshot();
}

terminalCommandField.addEventListener("change", async () => {
  const line = terminalCommandField.value.trim();
  // The line goes over whole and is split where quotes say to split it. Doing
  // that here on whitespace was the bug: `"/opt/my tools/zo"` arrived as two
  // words, neither of which exists.
  terminalCommand = line;
  const saved = await commitSetting(
    "terminal_command",
    "set_terminal_command",
    { command: line },
    {
      onError: (error) => {
        // This refusal belongs beside the field as well as in the global
        // error landing zone. `say` keeps it intact through a locale change.
        say(el("terminal-command-note"), () => String(error));
      },
    },
  );
  if (saved !== undefined) {
      say(el("terminal-command-note"), () =>
        terminalCommand === ""
          ? t("terminal.usesShell", "내 셸을 씁니다.")
          : t("terminal.appliesNext", "다음에 여는 터미널부터 적용됩니다."));
      // The title bar names this command, so it changes when the command does.
      paintCommandLabel();
    }
});

/* Which section the dialog was opened on, or `null` for its top.
 *
 * Kept because "settings is open" cannot answer which control opened it: the
 * three that lead here raise the same scrim, and the sidebar's `you are here`
 * mark has to follow the section rather than the dialog. */
let settingsSection = null;
let settingsReturnFocus = null;

/* `focus` names the control this dialog should open on. Three controls used to
 * lead here and all three called this with one argument, so 자동화, 단축키 and
 * the gear were three names for the top of one dialog — which is the shape a
 * dead control has even when the thing it opens is real. */
function setSettingsOpen(on, focus = null) {
  const wasOpen = !settingsView.hidden;
  if (on && !wasOpen) {
    termScrollbackCustomRequested = false;
    termScrollbackCanonical = null;
    const active = document.activeElement;
    settingsReturnFocus = active instanceof HTMLElement && !settingsView.contains(active)
      ? active
      : null;
  }
  if (on) {
    // The finder floats on the same scrim; see `openPalette`.
    closePalette();
    // Settings is never a stop in the walk — the terminal sits under it, so
    // opening it over 작업 or 자동화 ends that page's stay and the close
    // records the return. This used to make an exception for the automations
    // pane, back when 자동화 meant a settings field rather than a screen.
    setTaskOpen(false);
    setAutoOpen(false);
    // 공간도 전체 화면이고 같은 층에 산다. 닫지 않으면 설정 위에 남아, 사람이
    // ⌘,를 누르고도 계속 워크스페이스 크기를 본다.
    setSpaceOpen(false);
    // Reopening settings is a storage read, not a repaint of whatever this
    // webview last remembers. One full snapshot refreshes every preference,
    // including values committed by another window while this page was shut.
    void refreshSettingsSnapshot({ reportError: true });
    paintSettingsKeys();
    paintLocalePicker();
    paintThemePicker();
    // Before `paintSettingsSearch`, which reads the rendered text to decide
    // what a query matches — an unpainted picker is a row nobody can find.
    paintTermPrefs();
    // The query does NOT survive the close, unlike the pane. A screen that
    // reopens already narrowed by something typed an hour ago is a screen
    // missing most of itself for a reason nobody can see.
    el("settings-search").value = "";
    paintSettingsSearch();
    // Re-read on open rather than once at boot: an agent installed while this
    // window was up is the ordinary case, and the scan is a PATH walk.
    refreshAgents();
    // Same reason, and one more: the file lives in the repository, so pulling
    // a branch changes what this section says without the window doing
    // anything.
    refreshProjectScripts();
    // The branch prefix rides along with it: one section, one open, one pair
    // of reads.
    void refreshWorktreePrefs();
    // And whether the login behind each account is still there — a directory
    // emptied by hand between two opens is exactly the case the row's second
    // line exists to report.
    refreshClaudeAccounts();
    // Codex holds accounts by its own mechanism and needs the same re-read —
    // and one more reason of its own: the row for the machine's own login is
    // read live out of `~/.codex`, so a `codex login` run in a terminal
    // changes what this list says without the window doing anything.
    refreshCodexAccounts();
    // And Google, for the sharpest version of that reason: `zo login google`
    // and `zo logout` both write the file this card reads, so the login can
    // change with the window doing nothing at all.
    refreshGoogleAccount();
    // Same for the hooks: the agents' own config files are edited by their own
    // tools too, and this screen is where a person comes to check.
    refreshHooks();
    // The pane the caller pointed at, or the one the screen was last on —
    // Orca keeps `activeSectionId` across closes the same way.
    showSettingsPane(PANE_OF[focus] ?? settingsPane);
  }
  settingsSection = on ? focus : null;
  settingsView.hidden = !on;
  if (on && focus) {
    const at = el(focus);
    at?.scrollIntoView({ block: "center" });
    // A no-op on the sections that are not focusable, which is the honest
    // outcome: bring it into view and leave the caret where it was.
    at?.focus({ preventScroll: true });
  } else if (on) {
    // Nothing in particular to land on, so the screen itself takes the focus.
    // Left on the sink, the caret would still be sitting in a terminal this
    // screen is covering.
    settingsView.scrollTop = 0;
    settingsView.focus({ preventScroll: true });
  }
  paintNavCurrent();
  if (!on && wasOpen) {
    const returnTo = settingsReturnFocus;
    settingsReturnFocus = null;
    if (!termFloat.hidden) return;
    if (
      returnTo?.isConnected
      && !returnTo.closest("[hidden]")
      && !returnTo.matches(":disabled")
    ) {
      returnTo.focus({ preventScroll: true });
    } else {
      keySink.focus();
    }
  }
}

/* Which shortcut's destination is on screen.
 *
 * The two rows above the project tree had no state at all: the one that opened
 * what you are looking at was drawn exactly like the one that did not, and a
 * navigation row that cannot say "you are here" reads as decoration. Written
 * as `aria-current` rather than as a class because that is the fact being
 * drawn, and somebody on a screen reader is owed it too. */
function paintNavCurrent() {
  const showing = {
    "nav-tasks": !taskView.hidden,
    // The section, not the dialog. The gear and 단축키 raise the same scrim, so
    // asking only whether settings is open put this mark on 자동화 whenever
    // either of those was pressed — a row claiming to be the destination
    // nobody chose.
    // The page, now that there is one. This used to read a settings section,
    // because 자동화 opened a field rather than a screen.
    "nav-automations": !autoView.hidden,
  };
  for (const [id, on] of Object.entries(showing)) {
    if (on) el(id).setAttribute("aria-current", "page");
    else el(id).removeAttribute("aria-current");
  }
  // Orca shows the history arrows on the terminal, 작업 and 자동화 and hides
  // them on the settings page (`shouldShowWorktreeHistoryControls`,
  // App-BaqTRjaA.js:37167-37169). 자동화 lives on the settings DOM here, so
  // the section decides, not the surface.
  const plainSettings = !settingsView.hidden && settingsSection !== "terminal-command";
  el("nav-back").hidden = plainSettings;
  el("nav-forward").hidden = plainSettings;
}

/* ---- the Task screen (작업) ----
 *
 * Orca's own is a 31,000-line issue/PR client spanning four sources
 * (docs/reverse/orca-ui-inventory.md 1-i). This is the measured shell: Jira's
 * connect flow, IPC surface and list geometry were read in full, so Jira is
 * built to that contract. GitHub/GitLab/Linear are not measured to the same
 * depth — each provider keeps its own network vocabulary while sharing the
 * panel state and worktree doors below. */
const taskView = el("task-view");

function paintRouteWidth() {
  const wide = !taskView.hidden || !autoView.hidden;
  el("workbench").classList.toggle("is-route-wide", wide);
  tabstrip.hidden = wide;
  el("run-command").hidden = wide;
  el("toggle-aside").hidden = wide;
}

/* 지금 화면에 선 판, 그리고 사람이 고른 소스.
 *
 * 둘인 것은 하나로는 거짓말을 하기 때문이다. 고른 소스를 감추면 화면은 남은
 * 판으로 물러서야 하지만, 그 물러섬이 「고른 것」을 덮어쓰면 감췄던 소스를
 * 되살려도 사람은 자기가 고른 적 없는 판 위에 남는다 — 창 둘이 서로의 목록을
 * 스치는 동안 실제로 그렇게 됐다(설정 동시성 시험이 지나는 그 길). 고른 것은
 * 여기 남고, 서는 것은 `paintTaskSources`가 정한다. */
let taskSource = "jira";
let chosenTaskSource = "jira";
let defaultTaskSource = "jira";

function setTaskOpen(on) {
  if (on) {
    closePalette();
    crossingPages = true;
    setSettingsOpen(false);
    // 작업 and 자동화 are two pages over one stage, so opening either ends the
    // other's stay. Under `crossingPages` so the close is read as a crossing
    // rather than as a return to the terminal — going straight from one page
    // to the other must not thread the terminal into the walk.
    setAutoOpen(false);
    setSpaceOpen(false);
    crossingPages = false;
    paintTaskSources();
    refreshJiraStatus();
    refreshLinearStatus();
    if (taskSource === "github") void paintGithubPanel();
    if (taskSource === "gitlab") void paintGitlabPanel();
    // 작업 is a stop in the titlebar walk — Orca's `openTaskPage` does
    // `recordViewVisit("tasks")`. Following an arrow does not re-record.
    recordNavVisit("tasks");
    // 이 화면이 그 투어의 자리다. 띄울지는 백엔드가 정한다 — 세션당 하나,
    // 한 번 본 것은 다시 없음, 그리고 마법사를 본 적 없는 프로필에는 영영.
    void askForTour("tasks");
  } else if (!taskView.hidden && !crossingPages && activeWorktreePath) {
    // Closing the page puts the workspace terminal back on the stage, and
    // the return is a stop of its own — Orca sets the view to terminal and
    // records the visit. Left unrecorded, the cursor stays on 작업 while
    // the stage shows the terminal, and the first 뒤로 is a dead click.
    recordNavVisit(activeWorktreePath);
  }
  taskView.hidden = !on;
  if (!on && !el("jira-create-scrim").hidden) closeJiraCreate();
  if (!on && !el("linear-create-scrim").hidden) closeLinearCreate();
  paintRouteWidth();
  paintNavCurrent();
  if (!on && termFloat.hidden) keySink.focus();
}

/* ---- second-brain Obsidian vault -----------------------------------------
 *
 * Obsidian is only the optional viewer. The backend owns discovery, bounded
 * counting and idempotent scaffolding; this card selects a folder and paints
 * that answer without walking the vault in the webview. */
const OBSIDIAN_URL = "https://obsidian.md";
let secondBrainReport = null;
let secondBrainLoading = false;
let secondBrainError = null;
let secondBrainGeneration = 0;

function normalizeSecondBrainReport(answer) {
  const vault = answer?.vault && typeof answer.vault === "object"
    ? {
        path: String(answer.vault.path ?? ""),
        exists: answer.vault.exists === true,
        setupComplete: answer.vault.setup_complete === true,
        rawItems: Number(answer.vault.raw_items ?? 0),
        wikiPages: Number(answer.vault.wiki_pages ?? 0),
        lastIngested: answer.vault.last_ingested == null
          ? null
          : String(answer.vault.last_ingested),
        countsCapped: answer.vault.counts_capped === true,
      }
    : null;
  return {
    savedPath: String(answer?.saved_path ?? ""),
    vaults: Array.isArray(answer?.vaults)
      ? answer.vaults
          .filter((row) => row && typeof row.path === "string" && row.path)
          .map((row) => ({
            name: String(row.name || basename(row.path)),
            path: row.path,
            open: row.open === true,
          }))
      : [],
    vault,
    obsidianInstalled: answer?.obsidian_installed === true,
    created: Array.isArray(answer?.created) ? answer.created.map(String) : [],
    quickCommandsAdded: Number(answer?.quick_commands_added ?? 0),
    guides: Array.isArray(answer?.guides)
      ? answer.guides
          .filter((row) => row && typeof row.path === "string")
          .map((row) => ({
            agent: String(row.agent ?? ""),
            label: String(row.label || row.agent || ""),
            path: row.path,
            state: String(row.state ?? "missing"),
            detail: String(row.detail ?? ""),
          }))
      : [],
    weeklyReviewEnabled: answer?.weekly_review_enabled === true,
    weeklyReview: answer?.weekly_review && typeof answer.weekly_review === "object"
      ? {
          registered: answer.weekly_review.registered === true,
          cronId: answer.weekly_review.cron_id == null ? null : String(answer.weekly_review.cron_id),
          schedule: answer.weekly_review.schedule == null ? null : String(answer.weekly_review.schedule),
          nextDueAt: Number.isFinite(Number(answer.weekly_review.next_due_at))
            && answer.weekly_review.next_due_at != null
            ? Number(answer.weekly_review.next_due_at)
            : null,
          scheduler: answer.weekly_review.scheduler == null ? null : String(answer.weekly_review.scheduler),
          registry: answer.weekly_review.registry == null ? null : String(answer.weekly_review.registry),
          outcome: answer.weekly_review.outcome == null ? null : String(answer.weekly_review.outcome),
          error: answer.weekly_review.error == null ? null : String(answer.weekly_review.error),
        }
      : null,
  };
}

/* 주간 리뷰 스위치와 그 영수증 줄. 스위치는 설정(`second_brain_weekly_review`)을
 * 비추고, 줄은 볼트의 zo 등록부가 답한 것을 — 어느 스케줄러가 언제 여는지 — 그대로
 * 말한다. 켜져 있는데 기록이 없으면 그렇다고 말한다; 켜진 스위치가 열리지 않을
 * 턴을 약속하는 것이 이 카드가 하지 않을 일이다. */
function paintSecondBrainWeeklyReview() {
  const box = el("second-brain-weekly-review");
  box.checked = secondBrainWeeklyReview;
  box.disabled = secondBrainLoading || !secondBrainReport?.savedPath;
  const line = el("second-brain-weekly-review-receipt");
  const receipt = secondBrainReport?.weeklyReview ?? null;
  if (!secondBrainWeeklyReview) {
    line.hidden = true;
    line.textContent = "";
    return;
  }
  line.hidden = false;
  if (receipt?.error) {
    say(line, () => t("settings.secondBrain.reviewFailed", "주간 리뷰 cron을 확인하지 못했습니다: {{detail}}", {
      detail: receipt.error,
    }));
  } else if (receipt?.registered) {
    say(line, () => t(
      "settings.secondBrain.reviewRegistered",
      "{{scheduler}} 스케줄러에 등록됨 · {{schedule}} (UTC) · 다음 실행 {{when}}",
      {
        scheduler: receipt.scheduler ?? "?",
        schedule: receipt.schedule ?? "?",
        when: receipt.nextDueAt === null ? "?" : new Date(receipt.nextDueAt * 1000).toLocaleString(),
      },
    ));
  } else {
    say(line, () => t("settings.secondBrain.reviewMissing", "볼트의 zo cron 등록부에 기록이 없습니다 · 스위치를 껐다 다시 켜 주세요"));
  }
}

/* 「전역 연결」 줄. 사람이 볼 수 없는 사실 하나를 말한다: 볼트 밖 어느
 * 프로젝트에서 판을 열어도 그 에이전트가 볼트를 아는가. 아는 근거는 그
 * 에이전트의 전역 지시문 파일 안에 우리가 관리하는 블록이 서 있다는 것이고,
 * 실패는 어느 파일인지까지 말해야 사람이 고칠 수 있다. */
function paintSecondBrainGuides() {
  const guides = secondBrainReport?.guides ?? [];
  const linked = guides.filter((row) => row.state === "linked");
  const line = el("second-brain-guides");
  if (linked.length > 0 && linked.length === guides.length) {
    say(line, () => t("settings.secondBrain.linked", "{{agents}}에 연결됨", {
      agents: linked.map((row) => row.label).join("·"),
    }));
  } else if (linked.length > 0) {
    say(line, () => t("settings.secondBrain.linkStale", "{{agents}}에만 연결됨 · 다시 연결 필요", {
      agents: linked.map((row) => row.label).join("·"),
    }));
  } else {
    say(line, () => t("settings.secondBrain.notLinked", "연결 안 됨"));
  }

  const trouble = el("second-brain-link-error");
  const failed = guides.filter((row) => row.state === "failed");
  trouble.hidden = failed.length === 0;
  trouble.textContent = failed
    .map((row) => t(
      "settings.secondBrain.linkFailed",
      "{{file}}에 세컨드 브레인 안내를 쓰지 못했습니다: {{detail}}",
      { file: row.path, detail: row.detail },
    ))
    .join(" ");
}

function paintSecondBrain() {
  const report = secondBrainReport;
  const vault = report?.vault ?? null;
  const path = vault?.path || secondBrainVault;
  el("second-brain-path").value = path;

  const detectedField = el("second-brain-detected-field");
  const detected = el("second-brain-detected");
  detected.replaceChildren();
  for (const row of report?.vaults ?? []) {
    const option = document.createElement("option");
    option.value = row.path;
    option.textContent = row.open
      ? t("settings.secondBrain.detectedOpen", "{{name}} (열려 있음)", { name: row.name })
      : row.name;
    detected.appendChild(option);
  }
  detectedField.hidden = detected.options.length === 0;
  if (!detectedField.hidden) {
    detected.value = (report.vaults ?? []).some((row) => row.path === path) ? path : "";
  }

  const standing = el("second-brain-status");
  if (secondBrainLoading) {
    standing.dataset.state = "checking";
    say(standing, () => t("settings.integrations.checking", "확인 중…"));
  } else if (secondBrainError !== null) {
    standing.dataset.state = "error";
    say(standing, () => t("settings.integrations.checkFailed", "확인 실패"));
  } else if (vault?.setupComplete) {
    standing.dataset.state = "connected";
    say(standing, () => t("settings.secondBrain.ready", "준비됨"));
  } else {
    standing.dataset.state = "unchecked";
    say(standing, () => path
      ? t("settings.secondBrain.needsSetup", "세팅 필요")
      : t("settings.secondBrain.chooseStatus", "폴더 선택 필요"));
  }

  el("second-brain-raw").textContent = String(vault?.rawItems ?? 0);
  el("second-brain-wiki").textContent = String(vault?.wikiPages ?? 0);
  say(el("second-brain-last"), () => vault?.lastIngested
    || t("settings.secondBrain.never", "아직 없음"));
  say(el("second-brain-obsidian"), () => report?.obsidianInstalled
    ? t("settings.integrations.installed", "설치됨")
    : t("settings.integrations.notInstalled", "설치되지 않음"));
  el("second-brain-optional").hidden = report === null || report.obsidianInstalled;
  el("second-brain-open").hidden = !report?.obsidianInstalled || !vault?.exists;
  /* 그림으로 가는 문. 세팅이 끝난 볼트에만 선다 — `wiki/`가 없는 폴더의 그래프는
   * 빈 화면이고, 그 빈 화면은 「세팅」 단추가 이미 설명하고 있다. */
  el("second-brain-graph").hidden = !vault?.setupComplete;
  paintKnowledgeEntry();

  const trouble = el("second-brain-error");
  trouble.hidden = secondBrainError === null;
  trouble.textContent = secondBrainError ?? "";
  el("second-brain-refresh").disabled = secondBrainLoading;
  el("second-brain-browse").disabled = secondBrainLoading;
  el("second-brain-new").disabled = secondBrainLoading;
  el("second-brain-setup").disabled = secondBrainLoading || !vault?.exists;
  paintSecondBrainGuides();
  // 저장된 볼트가 있어야 다시 연결할 것이 있다 — 아직 고르기만 한 폴더는
  // 어느 에이전트에게도 실리지 않는다.
  el("second-brain-link").disabled = secondBrainLoading || !secondBrainReport?.savedPath;
  paintSecondBrainWeeklyReview();
}

/* 이 카드가 백엔드에 묻는 모든 길은 같은 모양이다: 세대를 올리고, 기다리는
 * 동안 잠그고, 늦게 온 답은 버린다. 물음만 다르다. */
async function askSecondBrain(command, args = {}) {
  const generation = ++secondBrainGeneration;
  secondBrainLoading = true;
  secondBrainError = null;
  paintSecondBrain();
  try {
    const answer = await invoke(command, args);
    if (generation !== secondBrainGeneration) return;
    secondBrainReport = normalizeSecondBrainReport(answer);
  } catch (error) {
    if (generation !== secondBrainGeneration) return;
    secondBrainError = String(error);
  }
  if (generation !== secondBrainGeneration) return;
  secondBrainLoading = false;
  paintSecondBrain();
}

async function refreshSecondBrain(path = null) {
  await askSecondBrain("second_brain_status", { path });
}

async function chooseSecondBrainFolder() {
  try {
    const [path] = await openPathBrowser({
      mode: "folder",
      start: el("second-brain-path").value || null,
    });
    if (path) await refreshSecondBrain(path);
  } catch (error) {
    secondBrainError = String(error);
    paintSecondBrain();
  }
}

el("second-brain-browse").addEventListener("click", () => void chooseSecondBrainFolder());
el("second-brain-new").addEventListener("click", () => void chooseSecondBrainFolder());
el("second-brain-refresh").addEventListener("click", () => {
  void refreshSecondBrain(el("second-brain-path").value || null);
});
el("second-brain-detected").addEventListener("change", (event) => {
  if (event.target.value) void refreshSecondBrain(event.target.value);
});
el("second-brain-link").addEventListener("click", () => {
  // 아무것도 묻지 않는다: 다시 연결할 볼트는 이미 저장된 그것이다.
  if (!secondBrainLoading) void askSecondBrain("second_brain_link");
});
el("second-brain-install").addEventListener("click", () => openExternal(OBSIDIAN_URL));
el("second-brain-weekly-review").addEventListener("change", (event) => {
  // 그림이 먼저, 쓰기는 그 뒤 — 다른 스위치와 같다. 백엔드는 zo가 볼트의
  // 등록부에 기록을 앉힌 뒤에야 설정을 쓰고, 거절되면 스냅샷이 스위치를
  // 되돌린다. 성공하면 상태를 다시 물어 영수증 줄을 채운다.
  const enabled = event.target.checked;
  secondBrainWeeklyReview = enabled;
  paintSecondBrainWeeklyReview();
  void commitSetting(
    "second_brain_weekly_review",
    "set_second_brain_weekly_review",
    { enabled },
  ).then((snapshot) => {
    if (snapshot !== undefined) void refreshSecondBrain();
  });
});
el("second-brain-graph").addEventListener("click", () => {
  // 설정 시트를 덮고 무대로 — 사이드바의 줄이 지나는 그 문 그대로.
  leavePagesForStage();
  openKnowledgeGraph();
});
el("second-brain-open").addEventListener("click", () => {
  const path = el("second-brain-path").value;
  if (path) invoke("second_brain_open", { path }).catch(showError);
});
el("second-brain-setup").addEventListener("click", async () => {
  const path = el("second-brain-path").value;
  if (!path || secondBrainLoading) return;
  secondBrainLoading = true;
  secondBrainError = null;
  el("second-brain-result").hidden = true;
  paintSecondBrain();
  try {
    const answer = await invoke("second_brain_setup", { path });
    secondBrainReport = normalizeSecondBrainReport(answer);
    secondBrainVault = secondBrainReport.savedPath;
    const result = el("second-brain-result");
    say(result, () => t(
      "settings.secondBrain.setupDone",
      "볼트 세팅 완료 · 파일 {{files}}개 생성 · 빠른 명령 {{commands}}개 추가",
      {
        files: secondBrainReport.created.length,
        commands: secondBrainReport.quickCommandsAdded,
      },
    ));
    result.hidden = false;
    await refreshSettingsQuickCommands();
    await openProjectAt(secondBrainVault);
  } catch (error) {
    secondBrainError = String(error);
  }
  secondBrainLoading = false;
  paintSecondBrain();
});

paintSecondBrain();

/* ---- 자동화: scheduled work ----------------------------------------------
 *
 * Orca's `AutomationsPage` is a screen, and its record is "at this time, in
 * this workspace, hand this prompt to an agent" — schedule, workspace mode,
 * precheck and all. Our 자동화 door used to open a settings field naming the
 * command a new terminal runs, which was a misreading of the word: that
 * setting is about ⌘T, and has nothing to do with a schedule.
 *
 * The cadences and what each means in cron are measured
 * (`buildAutomationCronSchedule`) and live in Rust — the backend derives the
 * expression and the next run, so this screen never has a second opinion
 * about when something fires. */
const autoView = el("auto-view");

const CADENCES = [
  { id: "hourly", label: () => t("auto.hourly", "매시간") },
  { id: "daily", label: () => t("auto.daily", "매일") },
  { id: "weekdays", label: () => t("auto.weekdays", "평일") },
  { id: "weekly", label: () => t("auto.weekly", "매주") },
  { id: "custom", label: () => t("auto.custom", "사용자 cron") },
];

/* The days, numbered the way `day_of_week` numbers them — 0 is Sunday, which
 * is cron's own numbering and the backend's. A function rather than the table
 * this was, for `laneStateLabel`'s reason: a const of `t()` calls resolves at
 * load, before a language has been chosen. */
const WEEKDAY_COUNT = 7;

function weekdayName(at) {
  switch (at) {
    case 0:
      return t("auto.sun", "일요일");
    case 1:
      return t("auto.mon", "월요일");
    case 2:
      return t("auto.tue", "화요일");
    case 3:
      return t("auto.wed", "수요일");
    case 4:
      return t("auto.thu", "목요일");
    case 5:
      return t("auto.fri", "금요일");
    default:
      return t("auto.sat", "토요일");
  }
}

let automations = [];
let autoDraft = null;
let autoSelectedId = null;
let autoRuns = [];
let autoLoading = false;
let autoFilter = "all";
let autoSort = null;
let autoDetailTab = "overview";
let autoRunsRequest = 0;
let autoRefreshGeneration = 0;
let autoViewMode = "list";
let autoAllRuns = [];
const _calInitial = new Date();
let autoCalYear = _calInitial.getFullYear();
let autoCalMonth = _calInitial.getMonth();
let autoCalSelectedDate = _calInitial.toISOString().slice(0, 10);

const AUTO_TEMPLATES = {
  "repo-audit": () => ({
    name: t("auto.templateRepoTitle", "평일 저장소 감사"),
    prompt: t(
      "auto.templateRepoPrompt",
      "의존성, 실패한 테스트, 위험한 공개 변경과 누락된 검증을 확인해 요약해 주세요.",
    ),
    schedule: { cadence: "weekdays", time: "09:00", day_of_week: 1, custom: "" },
  }),
  "release-readiness": () => ({
    name: t("auto.templateReleaseTitle", "주간 출시 점검"),
    prompt: t(
      "auto.templateReleasePrompt",
      "현재 프로젝트에서 출시를 막는 위험, 실패한 검사와 남은 후속 작업을 정리해 주세요.",
    ),
    schedule: { cadence: "weekly", time: "09:00", day_of_week: 1, custom: "" },
  }),
  "change-review": () => ({
    name: t("auto.templateReviewTitle", "매일 변경 리뷰"),
    prompt: t(
      "auto.templateReviewPrompt",
      "최근 변경을 검토하고 정확성, 사용자 경험과 테스트 범위의 위험을 알려 주세요.",
    ),
    schedule: { cadence: "daily", time: "17:00", day_of_week: 1, custom: "" },
  }),
  "queue-check": () => ({
    name: t("auto.templateQueueTitle", "시간별 대기열 점검"),
    prompt: t(
      "auto.templateQueuePrompt",
      "중단된 작업, 오래된 생성 파일과 실패한 로컬 검증을 찾아 우선순위대로 정리해 주세요.",
    ),
    schedule: { cadence: "hourly", time: "00:00", day_of_week: 1, custom: "" },
  }),
  /* A QA pass that leaves proof. The run is handed its own evidence folder,
   * so "save screenshots" has an answer to "where", and the shell is retired
   * when the agent reports done — a daily job should not leave yesterday's
   * pane listening. */
  "emulator-qa": () => ({
    name: t("auto.templateQaTitle", "매일 에뮬레이터 QA"),
    prompt: t(
      "auto.templateQaPrompt",
      "zerocode-emulator로 앱의 핵심 흐름을 처음부터 끝까지 실행해 회귀를 확인하세요. 동작마다 스크린샷이 증거 폴더에 자동으로 남습니다. 실패한 단계는 재현 순서와 함께 기록하고, 마지막에 증거 폴더에 report.md(단계별 판정)를 쓴 뒤 결과를 요약해 주세요.",
    ),
    schedule: { cadence: "daily", time: "09:30", day_of_week: 1, custom: "" },
    close_when_done: true,
  }),
  "raw-ingest": () => ({
    name: t("auto.templateRawTitle", "raw 자동 취합"),
    workspace: secondBrainVault || secondBrainReport?.vault?.path || activeWorktreePath || "",
    prompt: t(
      "auto.templateRawPrompt",
      "raw/에 있지만 wiki/에 아직 반영되지 않은 항목을 모두 취합하고 wiki/log.md에 기록해 줘.",
    ),
    schedule: { cadence: "daily", time: "19:00", day_of_week: 1, custom: "" },
    evidence: "prompt",
  }),
};

function selectedAutomation() {
  return automations.find((row) => row.id === autoSelectedId) ?? null;
}

async function refreshAutomations() {
  const generation = ++autoRefreshGeneration;
  autoLoading = true;
  paintAutomations();
  try {
    if (agentRows.length === 0) await refreshAgents();
    const rows = (await invoke("list_automations")) ?? [];
    if (generation !== autoRefreshGeneration) return;
    automations = rows;
  } catch (error) {
    if (generation !== autoRefreshGeneration) return;
    showError(error);
    automations = [];
  } finally {
    if (generation === autoRefreshGeneration) autoLoading = false;
  }
  if (generation !== autoRefreshGeneration) return;
  if (autoSelectedId && !selectedAutomation()) autoSelectedId = null;
  paintAutomations();
  paintAutomationDetail();
  if (selectedAutomation()) void refreshAutomationRuns();
  void refreshAllAutomationRuns();
}

async function refreshAllAutomationRuns() {
  if (automations.length === 0) {
    autoAllRuns = [];
    return;
  }
  try {
    const promises = automations.map((a) =>
      invoke("list_automation_runs", { automationId: a.id }).catch(() => [])
    );
    const results = await Promise.all(promises);
    autoAllRuns = results.flat();
    if (autoViewMode === "calendar") {
      paintAutomationCalendar();
    }
  } catch (_) {
    autoAllRuns = [];
  }
}

/* When a job next fires, in this reader's own words. The backend answers in
 * local minutes since the epoch (where UTC epoch values encode local wall time).
 * We format with timeZone: "UTC" to prevent the browser from double-applying
 * the local timezone offset. */
function whenNext(row) {
  if (!row.enabled) return t("auto.off", "꺼짐");
  if (row.next_run_at === null || row.next_run_at === undefined) {
    return t("auto.never", "예정 없음");
  }
  const when = new Date(row.next_run_at * 60000);
  // The backend already holds local wall-clock minutes. Formatting with
  // timeZone: "UTC" preserves the local numbers verbatim instead of shifting them.
  return when.toLocaleString(undefined, {
    timeZone: "UTC",
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

function automationScheduleText(row) {
  const schedule = row.schedule ?? {};
  const time = schedule.time || "00:00";
  switch (schedule.cadence) {
    case "hourly":
      return t("auto.whenHourly", "매시간 {{minute}}분", { minute: time.split(":")[1] ?? "00" });
    case "weekdays":
      return t("auto.whenWeekdays", "평일 {{time}}", { time });
    case "weekly":
      return t("auto.whenWeekly", "매주 {{day}} {{time}}", {
        day: weekdayName(Number(schedule.day_of_week ?? 1)), time,
      });
    case "custom":
      return schedule.custom || row.cron || t("auto.never", "예정 없음");
    default:
      return t("auto.whenDaily", "매일 {{time}}", { time });
  }
}

function autoLastRun(row) {
  if (row.last_run_at === null || row.last_run_at === undefined) return t("auto.neverRun", "실행 없음");
  return new Date(row.last_run_at * 60000).toLocaleString(undefined, {
    timeZone: "UTC",
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

function automationAgentLabel(row) {
  if (row.agent) return agentName(row.agent);
  if (row.command?.trim()) return t("auto.customCommand", "사용자 명령");
  return t("auto.noAgent", "에이전트 없음");
}

function autoVisibleRows() {
  const query = el("auto-search").value.trim().toLocaleLowerCase();
  const rows = automations.filter((row) => {
    if (autoFilter === "enabled" && !row.enabled) return false;
    if (autoFilter === "paused" && row.enabled) return false;
    if (!query) return true;
    return [row.name, row.prompt, row.workspace, row.agent && agentName(row.agent)]
      .filter(Boolean)
      .some((word) => String(word).toLocaleLowerCase().includes(query));
  });
  if (!autoSort) return rows;
  return rows.slice().sort((left, right) => {
    const a = autoSort.field === "name" ? left.name ?? "" : left.last_run_at ?? -Infinity;
    const b = autoSort.field === "name" ? right.name ?? "" : right.last_run_at ?? -Infinity;
    const compared = typeof a === "string" ? a.localeCompare(b) : a - b;
    return compared * autoSort.direction;
  });
}

function paintAutomations() {
  const initialLoading = autoLoading && automations.length === 0;
  el("auto-loading").hidden = !initialLoading;
  el("auto-refresh").classList.toggle("is-loading", autoLoading);
  el("auto-empty").hidden = initialLoading || automations.length > 0;
  el("auto-catalog").hidden = initialLoading || automations.length === 0;

  const isCal = autoViewMode === "calendar";
  const tableEl = el("auto-table");
  const calEl = el("auto-calendar");
  const listBtn = el("auto-view-list-btn");
  const calBtn = el("auto-view-cal-btn");
  if (tableEl) tableEl.hidden = isCal;
  if (calEl) calEl.hidden = !isCal;
  if (listBtn) listBtn.classList.toggle("is-active", !isCal);
  if (calBtn) calBtn.classList.toggle("is-active", isCal);

  if (isCal) {
    paintAutomationCalendar();
  }

  const list = el("auto-list");
  list.replaceChildren();
  const rows = autoVisibleRows();
  for (const row of rows) {
    const item = document.createElement("div");
    item.className = "auto-row";
    item.setAttribute("role", "row");
    item.tabIndex = 0;
    item.setAttribute("aria-selected", row.id === autoSelectedId ? "true" : "false");
    item.classList.toggle("is-active", row.id === autoSelectedId);
    item.classList.toggle("is-off", !row.enabled);
    item.innerHTML =
      '<span class="auto-row-name" role="cell"></span>' +
      '<span class="auto-row-schedule" role="cell"></span>' +
      '<span class="auto-row-project" role="cell"></span>' +
      '<span class="auto-row-when" role="cell"></span>' +
      '<span class="auto-row-last" role="cell"></span>' +
      '<button class="auto-row-toggle" type="button" role="switch"></button>' +
      '<span class="auto-row-agent" role="cell"></span>' +
      '<span class="auto-row-actions" role="cell"></span>';
    item.querySelector(".auto-row-name").textContent = row.name || t("auto.untitled", "이름 없음");
    item.querySelector(".auto-row-schedule").textContent = automationScheduleText(row);
    item.querySelector(".auto-row-project").textContent = basename(row.workspace);
    item.querySelector(".auto-row-when").textContent = whenNext(row);
    const last = item.querySelector(".auto-row-last");
    last.textContent = autoLastRun(row);
    const runs = document.createElement("span");
    runs.className = "auto-row-runs";
    runs.hidden = !(row.runs > 0);
    if (!runs.hidden) runs.textContent = t("auto.runCount", "런 {{n}}회", { n: row.runs });
    last.appendChild(runs);
    item.querySelector(".auto-row-agent").textContent = automationAgentLabel(row);
    const toggle = item.querySelector(".auto-row-toggle");
    toggle.setAttribute("aria-checked", row.enabled ? "true" : "false");
    toggle.setAttribute("aria-label", row.enabled
      ? t("auto.pause", "일시정지")
      : t("auto.resume", "다시 시작"));
    toggle.addEventListener("click", (event) => {
      event.stopPropagation();
      toggle.disabled = true;
      invoke("enable_automation", { id: row.id, enabled: !row.enabled })
        .then((rows) => {
          automations = rows;
          paintAutomations();
          paintAutomationDetail();
        })
        .catch(showError)
        .finally(() => { toggle.disabled = false; });
    });
    const actions = item.querySelector(".auto-row-actions");
    const run = document.createElement("button");
    run.className = "auto-row-action";
    run.type = "button";
    run.dataset.tip = t("auto.runNow", "지금 실행");
    run.setAttribute("aria-label", run.dataset.tip);
    run.innerHTML = '<svg class="icon" aria-hidden="true"><use href="#i-play"></use></svg>';
    run.addEventListener("click", (event) => {
      event.stopPropagation();
      void runAutomationNow(row);
    });
    const edit = document.createElement("button");
    edit.className = "auto-row-action";
    edit.type = "button";
    edit.dataset.tip = t("app.edit", "편집");
    edit.setAttribute("aria-label", edit.dataset.tip);
    edit.innerHTML = '<svg class="icon" aria-hidden="true"><use href="#i-pencil"></use></svg>';
    edit.addEventListener("click", (event) => {
      event.stopPropagation();
      editAutomation(row);
    });
    actions.append(run, edit);
    const open = () => selectAutomation(row.id);
    item.addEventListener("click", open);
    item.addEventListener("keydown", (event) => {
      if (event.key === "Enter" || event.key === " ") {
        event.preventDefault();
        open();
      }
    });
    list.appendChild(item);
  }
  el("auto-no-match").hidden = rows.length > 0 || automations.length === 0;
}

function paintAutomationCalendar() {
  const calDays = el("auto-cal-days");
  const titleEl = el("auto-cal-title");
  if (!calDays || !titleEl) return;

  titleEl.textContent = t("auto.yearMonth", "{{year}}년 {{month}}월", {
    year: autoCalYear,
    month: autoCalMonth + 1,
  });

  const today = new Date();
  const todayMonthPadded = String(today.getMonth() + 1).padStart(2, "0");
  const todayDayPadded = String(today.getDate()).padStart(2, "0");
  const todayStr = [today.getFullYear(), todayMonthPadded, todayDayPadded].join("-");

  const firstDay = new Date(autoCalYear, autoCalMonth, 1).getDay(); // 0: Sun ... 6: Sat
  const daysInMonth = new Date(autoCalYear, autoCalMonth + 1, 0).getDate();
  const prevDaysInMonth = new Date(autoCalYear, autoCalMonth, 0).getDate();

  calDays.replaceChildren();

  const totalCells = Math.ceil((firstDay + daysInMonth) / 7) * 7;
  const activeRows = autoVisibleRows();

  for (let i = 0; i < totalCells; i++) {
    const cell = document.createElement("div");
    cell.className = "auto-cal-cell";
    cell.setAttribute("role", "gridcell");

    let cellYear = autoCalYear;
    let cellMonth = autoCalMonth;
    let dayNum = 0;

    if (i < firstDay) {
      dayNum = prevDaysInMonth - firstDay + i + 1;
      cellMonth = autoCalMonth - 1;
      if (cellMonth < 0) {
        cellMonth = 11;
        cellYear--;
      }
      cell.classList.add("is-other-month");
    } else if (i < firstDay + daysInMonth) {
      dayNum = i - firstDay + 1;
    } else {
      dayNum = i - firstDay - daysInMonth + 1;
      cellMonth = autoCalMonth + 1;
      if (cellMonth > 11) {
        cellMonth = 0;
        cellYear++;
      }
      cell.classList.add("is-other-month");
    }

    const cellMonthPadded = String(cellMonth + 1).padStart(2, "0");
    const cellDayPadded = String(dayNum).padStart(2, "0");
    const dateStr = [cellYear, cellMonthPadded, cellDayPadded].join("-");
    const dayOfWeek = (firstDay + (i - firstDay)) % 7; // 0: Sun ... 6: Sat

    if (dateStr === todayStr) {
      cell.classList.add("is-today");
    }
    if (dateStr === autoCalSelectedDate) {
      cell.classList.add("is-selected");
    }

    const head = document.createElement("div");
    head.className = "auto-cal-cell-head";
    const numSpan = document.createElement("span");
    numSpan.className = "auto-cal-date-num";
    numSpan.textContent = String(dayNum);
    head.appendChild(numSpan);

    // 실행 이력 히트맵 점 표시 (해당 날짜에 실행된 이력)
    const historyRunsOnDate = autoAllRuns.filter((run) => {
      if (!run.at_epoch_ms) return false;
      const rd = new Date(run.at_epoch_ms);
      const rMonthPadded = String(rd.getUTCMonth() + 1).padStart(2, "0");
      const rDayPadded = String(rd.getUTCDate()).padStart(2, "0");
      const rDateStr = [rd.getUTCFullYear(), rMonthPadded, rDayPadded].join("-");
      return rDateStr === dateStr;
    });

    if (historyRunsOnDate.length > 0) {
      const dots = document.createElement("div");
      dots.className = "auto-cal-dots";
      const hasSuccess = historyRunsOnDate.some((r) => !r.failure);
      const hasFail = historyRunsOnDate.some((r) => r.failure);
      if (hasSuccess) {
        const dot = document.createElement("span");
        dot.className = "auto-cal-dot is-success";
        dot.dataset.tip = t("auto.hasSuccess", "성공한 실행 있음");
        dots.appendChild(dot);
      }
      if (hasFail) {
        const dot = document.createElement("span");
        dot.className = "auto-cal-dot is-fail";
        dot.dataset.tip = t("auto.hasFail", "실패한 실행 있음");
        dots.appendChild(dot);
      }
      head.appendChild(dots);
    }
    cell.appendChild(head);

    const chipsContainer = document.createElement("div");
    chipsContainer.className = "auto-cal-chips";

    // Find automations scheduled on this day
    const matching = [];
    for (const row of activeRows) {
      if (!row.enabled) continue;
      const sched = row.schedule ?? {};
      const time = sched.time || "00:00";
      let fires = false;
      if (sched.cadence === "daily" || sched.cadence === "hourly") {
        fires = true;
      } else if (sched.cadence === "weekdays") {
        fires = dayOfWeek >= 1 && dayOfWeek <= 5;
      } else if (sched.cadence === "weekly") {
        fires = Number(sched.day_of_week ?? 1) === dayOfWeek;
      } else if (row.next_run_at) {
        const nextD = new Date(row.next_run_at * 60000);
        const nextMonthPadded = String(nextD.getUTCMonth() + 1).padStart(2, "0");
        const nextDayPadded = String(nextD.getUTCDate()).padStart(2, "0");
        const nextStr = [nextD.getUTCFullYear(), nextMonthPadded, nextDayPadded].join("-");
        fires = (nextStr === dateStr);
      }
      if (fires) {
        matching.push({ row, time });
      }
    }

    for (const { row, time } of matching.slice(0, 3)) {
      const chip = document.createElement("div");
      chip.className = "auto-cal-chip";
      const rowName = row.name || t("auto.untitled", "이름 없음");
      chip.dataset.tip = `${time} ${rowName}`;

      const timeSpan = document.createElement("span");
      timeSpan.className = "auto-cal-chip-time";
      timeSpan.textContent = time;

      const nameSpan = document.createElement("span");
      nameSpan.className = "auto-cal-chip-name";
      nameSpan.textContent = rowName;

      chip.append(timeSpan, nameSpan);
      chip.addEventListener("click", (e) => {
        e.stopPropagation();
        selectAutomation(row.id);
      });
      chipsContainer.appendChild(chip);
    }
    if (matching.length > 3) {
      const more = document.createElement("div");
      more.className = "auto-cal-chip";
      more.textContent = t("auto.moreCount", "+{{n}}개 더", { n: matching.length - 3 });
      chipsContainer.appendChild(more);
    }

    cell.appendChild(chipsContainer);

    cell.addEventListener("click", () => {
      autoCalSelectedDate = dateStr;
      paintAutomationCalendar();
    });

    // 빈 셀 더블클릭 시 해당 요일/일정으로 새 자동화 작성 폼 열기
    cell.addEventListener("dblclick", (e) => {
      e.stopPropagation();
      editAutomation(null, {
        schedule: {
          cadence: (dayOfWeek >= 1 && dayOfWeek <= 5) ? "weekdays" : "weekly",
          time: "09:00",
          day_of_week: dayOfWeek,
          custom: "",
        },
      });
    });

    calDays.appendChild(cell);
  }

  paintCalendarSelectedPanel(activeRows);
}

function paintCalendarSelectedPanel(rows) {
  const panel = el("auto-cal-selected-panel");
  const title = el("auto-cal-selected-title");
  const list = el("auto-cal-selected-list");
  if (!panel || !title || !list) return;

  const [y, m, d] = autoCalSelectedDate.split("-").map(Number);
  const selectedDate = new Date(y, m - 1, d);
  const dayOfWeek = selectedDate.getDay();
  const dayStr = weekdayName(dayOfWeek);
  const selectedDateLabel = [y, m, d].join("-") + " (" + dayStr + ")";

  title.textContent = t("auto.selectedDateSchedule", "{{date}} 일정 및 이력", {
    date: selectedDateLabel,
  });
  list.replaceChildren();

  const matching = [];
  for (const row of rows) {
    if (!row.enabled) continue;
    const sched = row.schedule ?? {};
    const time = sched.time || "00:00";
    let fires = false;
    if (sched.cadence === "daily" || sched.cadence === "hourly") {
      fires = true;
    } else if (sched.cadence === "weekdays") {
      fires = dayOfWeek >= 1 && dayOfWeek <= 5;
    } else if (sched.cadence === "weekly") {
      fires = Number(sched.day_of_week ?? 1) === dayOfWeek;
    } else if (row.next_run_at) {
      const nextD = new Date(row.next_run_at * 60000);
      const nextMonthPadded = String(nextD.getUTCMonth() + 1).padStart(2, "0");
      const nextDayPadded = String(nextD.getUTCDate()).padStart(2, "0");
      const nextStr = [nextD.getUTCFullYear(), nextMonthPadded, nextDayPadded].join("-");
      fires = (nextStr === autoCalSelectedDate);
    }
    if (fires) {
      matching.push({ row, time });
    }
  }

  if (matching.length === 0) {
    const empty = document.createElement("p");
    empty.className = "auto-no-match";
    empty.style.padding = "var(--space-3) 0";
    empty.textContent = t("auto.noScheduledOnDate", "선택한 날짜에 예정된 자동화가 없습니다.");
    list.appendChild(empty);
  } else {
    for (const { row, time } of matching) {
      const item = document.createElement("div");
      item.className = "auto-cal-selected-item";

      const left = document.createElement("div");
      left.className = "auto-cal-selected-left";

      const timeSpan = document.createElement("span");
      timeSpan.className = "auto-cal-selected-time";
      timeSpan.textContent = time;

      const info = document.createElement("div");
      info.className = "auto-cal-selected-info";

      const nameSpan = document.createElement("span");
      nameSpan.className = "auto-cal-selected-name";
      nameSpan.textContent = row.name || t("auto.untitled", "이름 없음");

      const subSpan = document.createElement("span");
      subSpan.className = "auto-cal-selected-sub";
      subSpan.textContent = `${automationScheduleText(row)} · ${basename(row.workspace)} · ${automationAgentLabel(row)}`;

      info.append(nameSpan, subSpan);
      left.append(timeSpan, info);

      const openBtn = document.createElement("button");
      openBtn.className = "btn btn--secondary auto-cal-detail-btn";
      openBtn.type = "button";
      openBtn.textContent = t("auto.runOpen", "보기");
      openBtn.addEventListener("click", () => {
        selectAutomation(row.id);
      });

      item.append(left, openBtn);
      list.appendChild(item);
    }
  }

  // 과거 실행 이력 목록 표시
  const historyRunsOnSelected = autoAllRuns.filter((run) => {
    if (!run.at_epoch_ms) return false;
    const rd = new Date(run.at_epoch_ms);
    const rMonthPadded = String(rd.getUTCMonth() + 1).padStart(2, "0");
    const rDayPadded = String(rd.getUTCDate()).padStart(2, "0");
    const rDateStr = [rd.getUTCFullYear(), rMonthPadded, rDayPadded].join("-");
    return rDateStr === autoCalSelectedDate;
  });

  if (historyRunsOnSelected.length > 0) {
    const histSection = document.createElement("div");
    histSection.className = "auto-cal-history-section";
    const histTitle = document.createElement("h5");
    histTitle.className = "auto-cal-history-title";
    histTitle.textContent = t("auto.historyRunsCount", "실행 이력 ({{n}}건)", { n: historyRunsOnSelected.length });
    histSection.appendChild(histTitle);

    for (const run of historyRunsOnSelected) {
      const parentAuto = automations.find((a) => a.id === run.automation_id);
      const isFail = Boolean(run.failure);
      const hItem = document.createElement("div");
      hItem.className = "auto-cal-selected-item";

      const left = document.createElement("div");
      left.className = "auto-cal-selected-left";

      const dot = document.createElement("span");
      dot.className = `auto-cal-dot ${isFail ? "is-fail" : "is-success"}`;
      dot.style.width = "8px";
      dot.style.height = "8px";
      dot.style.flex = "none";

      const info = document.createElement("div");
      info.className = "auto-cal-selected-info";

      const nameSpan = document.createElement("span");
      nameSpan.className = "auto-cal-selected-name";
      nameSpan.textContent = `${parentAuto?.name || t("auto.untitled", "이름 없음")} (${runClock(run.at_epoch_ms)})`;

      const subSpan = document.createElement("span");
      subSpan.className = "auto-cal-selected-sub";
      subSpan.textContent = `${isFail ? t("auto.runError", "실행 실패") : t("auto.runSuccess", "실행 완료")} · ${basename(run.root || "")}`;

      info.append(nameSpan, subSpan);
      left.append(dot, info);

      const openBtn = document.createElement("button");
      openBtn.className = "btn btn--secondary auto-cal-detail-btn";
      openBtn.type = "button";
      openBtn.textContent = t("auto.runOpen", "보기");
      openBtn.addEventListener("click", () => {
        if (run.automation_id) selectAutomation(run.automation_id);
      });

      hItem.append(left, openBtn);
      histSection.appendChild(hItem);
    }
    list.appendChild(histSection);
  }
}

function selectAutomation(id) {
  autoSelectedId = id;
  autoDetailTab = "overview";
  autoRuns = [];
  paintAutomations();
  paintAutomationDetail();
  void refreshAutomationRuns();
}

function setAutoDetailTab(tab) {
  autoDetailTab = tab;
  const runs = tab === "runs";
  el("auto-overview").hidden = runs;
  el("auto-runs").hidden = !runs;
  el("auto-overview-tab").classList.toggle("is-active", !runs);
  el("auto-overview-tab").setAttribute("aria-selected", String(!runs));
  el("auto-runs-tab").classList.toggle("is-active", runs);
  el("auto-runs-tab").setAttribute("aria-selected", String(runs));
}

/* What the job's evidence setting comes to for THIS prompt: the policy is a
 * word, but "프롬프트에 따라" is only useful once it says which way. */
function evidencePolicyLabel(row) {
  if (row.evidence === "always") return t("auto.evidenceAlways", "항상 남김");
  if (row.evidence === "never") return t("auto.evidenceNever", "남기지 않음");
  return row.leaves_evidence
    ? t("auto.evidencePromptYes", "프롬프트에 따라 — 이 프롬프트는 남김")
    : t("auto.evidencePromptNo", "프롬프트에 따라 — 이 프롬프트는 안 남김");
}

function graceLabel(minutes) {
  if (!minutes) return t("auto.noGrace", "유예 없음");
  if (minutes < 60) return t("auto.minutes", "{{n}}분", { n: minutes });
  return t("auto.hours", "{{n}}시간", { n: minutes / 60 });
}

function paintAutomationDetail() {
  const row = selectedAutomation();
  el("auto-list-page").hidden = row !== null;
  el("auto-detail").hidden = row === null;
  if (!row) return;
  el("auto-detail-name").textContent = row.name || t("auto.untitled", "이름 없음");
  el("auto-detail-prompt").textContent = row.prompt || t("auto.noPrompt", "프롬프트 없음");
  const status = el("auto-detail-status");
  status.textContent = row.enabled ? t("auto.enabled", "활성") : t("auto.paused", "일시정지");
  status.classList.toggle("is-enabled", row.enabled);
  el("auto-detail-schedule").textContent = automationScheduleText(row);
  el("auto-detail-next").textContent = whenNext(row);
  el("auto-detail-workspace").textContent = row.workspace_mode === "new_per_run"
    ? t("auto.modeNewPerRun", "실행마다 새 워크트리")
    : row.workspace;
  el("auto-detail-session").textContent = row.reuse_session
    ? t("auto.reuse", "실행 중인 세션 재사용")
    : t("auto.freshSession", "매번 새 세션");
  el("auto-detail-grace").textContent = graceLabel(row.missed_run_grace_minutes);
  el("auto-detail-precheck").textContent = row.precheck || t("auto.none", "없음");
  el("auto-detail-agent").textContent = automationAgentLabel(row);
  el("auto-detail-after").textContent = row.close_when_done
    ? t("auto.closeWhenDone", "완료되면 터미널 닫기")
    : t("auto.keepPaneOpen", "터미널을 열어 둠");
  el("auto-detail-evidence").textContent = evidencePolicyLabel(row);
  el("auto-detail-toggle").textContent = row.enabled
    ? t("auto.pause", "일시정지")
    : t("auto.resume", "다시 시작");
  el("auto-runs-count").textContent = String(autoRuns.length || row.runs || 0);
  setAutoDetailTab(autoDetailTab);
}

async function refreshAutomationRuns() {
  const row = selectedAutomation();
  const request = ++autoRunsRequest;
  if (!row) {
    autoRuns = [];
    paintAutomationRuns();
    return;
  }
  try {
    const rows = (await invoke("list_automation_runs", { automationId: row.id })) ?? [];
    if (request !== autoRunsRequest || row.id !== autoSelectedId) return;
    autoRuns = rows;
  } catch (error) {
    showError(error);
    autoRuns = [];
  }
  paintAutomationRuns();
  paintAutomationDetail();
}

function paintAutomationRuns() {
  const list = el("auto-runs-list");
  list.replaceChildren();
  el("auto-runs-empty").hidden = autoRuns.length > 0;
  for (const run of autoRuns) {
    const item = document.createElement("div");
    item.className = "auto-run-row";
    item.innerHTML = '<span class="auto-run-when"></span><span class="auto-run-where"></span><span class="auto-run-state"></span>';
    item.querySelector(".auto-run-when").textContent = runClock(run.at_epoch_ms);
    item.querySelector(".auto-run-where").textContent = basename(run.root);
    const state = item.querySelector(".auto-run-state");
    if (run.failure) {
      state.className = "auto-run-ended auto-run-failed";
      state.textContent = t("auto.runError", "실행 실패");
      state.dataset.tip = [
        run.failure.stage,
        run.failure.error,
        run.failure.worktree_cleanup_error,
      ]
        .filter(Boolean)
        .join("\n");
    } else if (run.skipped) {
      const why = state;
      why.className = "auto-run-skipped";
      why.textContent = t("auto.runSkipped", "건너뜀 — precheck 실패");
      why.dataset.tip = [
        run.skipped.command,
        run.skipped.timed_out
          ? t("auto.runSkippedTimeout", "시간 초과")
          : `exit ${run.skipped.exit_code ?? "?"}`,
        run.skipped.output,
      ].filter(Boolean).join("\n");
    } else if (run.ended_at_epoch_ms) {
      state.className = "auto-run-ended";
      if (run.exit_code === 0) {
        state.textContent = t("auto.runDone", "완료");
      } else if (typeof run.exit_code === "number") {
        state.classList.add("auto-run-failed");
        state.textContent = t("auto.runFailed", "실패 — exit {{code}}", { code: run.exit_code });
      } else if (run.unwitnessed) {
        // Closed by the boot sweep: the last window left it open and this one
        // found no shell. Not a verdict on the work — the agent may well have
        // finished — only on what the window saw.
        state.classList.add("auto-run-unwitnessed");
        state.textContent = t("auto.runUnwitnessed", "종료 — 완료 보고 없음");
      } else {
        state.textContent = t("auto.runEnded", "종료됨");
      }
      // A run that ended cleanly, or whose code nobody read, says what it
      // left; a failed exit says only that it failed.
      if (run.exit_code === 0 || typeof run.exit_code !== "number") {
        state.textContent += evidenceSuffix(run);
      }
      state.dataset.tip = run.unwitnessed
        ? `${runClock(run.ended_at_epoch_ms)}\n${t(
            "auto.runUnwitnessedWhy",
            "창이 이 실행의 종료를 보지 못했습니다 — 재시작 뒤 판이 없었습니다",
          )}`
        : runClock(run.ended_at_epoch_ms);
    } else if (run.completed_at_epoch_ms) {
      // The agent said its turn ended; the shell is still there to look at.
      // A run that was asked for proof and left none says so, in the colour
      // this window says "that did not work" in — "성공했습니다" is not it.
      state.className = "auto-run-ended auto-run-completed";
      if (run.evidence_dir && !(run.evidence_files > 0)) {
        state.classList.add("auto-run-noproof");
        state.textContent = t("auto.runCompletedNoEvidence", "완료 — 증거 없음");
      } else {
        state.textContent = t("auto.runCompleted", "완료 — 터미널 열림") + evidenceSuffix(run);
      }
      state.dataset.tip = runClock(run.completed_at_epoch_ms);
    } else {
      state.className = "auto-run-ended";
      state.textContent = t("auto.running", "실행 중");
    }
    const actions = document.createElement("span");
    actions.className = "auto-run-actions";
    const tab = run.skipped ? null : tabOfTerm(run.term);
    const openableTab = run.failure ? null : tab;
    if (openableTab) {
      const open = document.createElement("button");
      open.className = "btn auto-run-open";
      open.type = "button";
      open.textContent = t("auto.runOpen", "보기");
      open.addEventListener("click", () => {
        setAutoOpen(false);
        void adoptAutomationRun(run, { focus: true });
      });
      actions.appendChild(open);
    }
    let detail = null;
    if (run.result || run.evidence_dir) {
      detail = document.createElement("div");
      detail.className = "auto-run-detail";
    }
    if (run.result) {
      const result = document.createElement("p");
      result.className = "auto-run-result";
      result.textContent = run.result;
      detail.appendChild(result);
    }
    if (run.evidence_dir) {
      const gallery = document.createElement("div");
      gallery.className = "auto-run-evidence";
      gallery.hidden = true;
      detail.appendChild(gallery);
      const evidence = document.createElement("button");
      evidence.className = "btn auto-run-evidence-toggle";
      evidence.type = "button";
      evidence.textContent = run.evidence_files > 0
        ? t("auto.evidenceCount", "증거 {{n}}개", { n: run.evidence_files })
        : t("auto.evidence", "증거");
      evidence.setAttribute("aria-expanded", "false");
      evidence.addEventListener("click", () => {
        void toggleRunEvidence(run, gallery, evidence);
      });
      actions.appendChild(evidence);
    }
    // NewPerRun 등 작업 완료 워크트리의 원클릭 머지 & 정리(Squash & Delete).
    // 병합할 것은 이 실행이 **만든** 워크트리다 — 프로젝트 자리에서 그냥 돈
    // 실행에는 병합할 가지도, 걷을 워크트리도 없다.
    if (run.made_worktree && !run.failure && !run.skipped) {
      const mergeBtn = document.createElement("button");
      mergeBtn.className = "btn auto-run-merge-btn";
      mergeBtn.type = "button";
      mergeBtn.textContent = t("auto.mergeAndClean", "병합 및 정리");
      mergeBtn.dataset.tip = t("auto.mergeAndCleanTip", "메인 브랜치에 스쿼시 병합하고 워크트리를 정리합니다");
      mergeBtn.addEventListener("click", async () => {
        const confirmed = await askConfirm({
          title: t("auto.mergeConfirmTitle", "워크트리 병합 및 정리"),
          body: t(
            "auto.mergeConfirmBody",
            "{{root}} 워크트리의 변경 사항을 메인에 병합(Squash)하고 워크트리를 안전하게 삭제하시겠습니까?",
            { root: basename(run.root) },
          ),
          confirm: t("auto.mergeConfirmButton", "병합 및 삭제"),
          deny: t("app.cancel", "취소"),
          danger: false,
        });
        if (!confirmed) return;
        mergeBtn.disabled = true;
        mergeBtn.textContent = t("auto.merging", "병합 중…");
        try {
          const res = await invoke("merge_and_remove_worktree", { path: run.root });
          toast(String(res));
          refreshWorktrees();
          void refreshAutomationRuns();
        } catch (err) {
          showError(err);
          mergeBtn.disabled = false;
          mergeBtn.textContent = t("auto.mergeAndClean", "병합 및 정리");
        }
      });
      actions.appendChild(mergeBtn);
    }
    item.appendChild(actions);
    if (detail) item.appendChild(detail);
    list.appendChild(item);
  }
}

/* " · 증거 N개" after a state word, or nothing when the run left none. */
function evidenceSuffix(run) {
  return run.evidence_files > 0
    ? ` · ${t("auto.evidenceCount", "증거 {{n}}개", { n: run.evidence_files })}`
    : "";
}

function runClock(epochMs) {
  return new Date(epochMs).toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "numeric",
    minute: "2-digit",
  });
}

function bytesLabel(bytes) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(bytes < 10240 ? 1 : 0)} KB`;
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`;
}

/* What a run left in its evidence folder, fetched when asked for and let go
 * when folded — a day of screenshots is fetched as previews, and a hundred
 * rows of them must not sit in the document for a list nobody opened. */
async function toggleRunEvidence(run, gallery, button) {
  if (!gallery.hidden) {
    gallery.hidden = true;
    gallery.replaceChildren();
    button.setAttribute("aria-expanded", "false");
    return;
  }
  button.disabled = true;
  let evidence;
  try {
    evidence = await invoke("list_run_evidence", { automationId: run.automation_id, runId: run.id });
  } catch (error) {
    showError(error);
    return;
  } finally {
    button.disabled = false;
  }
  gallery.replaceChildren();
  const head = document.createElement("div");
  head.className = "auto-run-evidence-head";
  const where = document.createElement("span");
  where.className = "auto-run-evidence-dir";
  where.textContent = evidence.dir;
  where.dataset.tip = evidence.dir;
  const reveal = document.createElement("button");
  reveal.className = "btn auto-run-evidence-reveal";
  reveal.type = "button";
  reveal.textContent = t("auto.evidenceReveal", "폴더 열기");
  reveal.addEventListener("click", () => {
    invoke("reveal_run_evidence", { automationId: run.automation_id, runId: run.id }).catch(showError);
  });
  head.append(where, reveal);
  gallery.appendChild(head);
  const steps = evidence.steps ?? [];
  // A frame is captioned by the step it proves — "3. emulator tap" says more
  // than the file name the window gave it.
  const stepOfShot = new Map(steps.filter((step) => step.shot).map((step) => [step.shot, step]));
  const stepWords = (step) => `${step.n}. ${step.tool} ${step.verb}`;
  if (evidence.files.length === 0) {
    const empty = document.createElement("p");
    empty.className = "auto-run-evidence-empty";
    empty.textContent = t("auto.evidenceEmpty", "남긴 파일이 없습니다.");
    gallery.appendChild(empty);
  } else {
    const grid = document.createElement("div");
    grid.className = "auto-run-evidence-grid";
    for (const file of evidence.files) {
      const step = stepOfShot.get(file.name);
      const caption = `${step ? stepWords(step) : file.name} · ${bytesLabel(file.bytes)}`;
      if (file.preview) {
        const figure = document.createElement("figure");
        figure.className = "auto-run-shot";
        const image = document.createElement("img");
        image.src = file.preview;
        image.alt = file.name;
        image.loading = "lazy";
        const label = document.createElement("figcaption");
        label.textContent = caption;
        label.dataset.tip = file.name;
        figure.append(image, label);
        grid.appendChild(figure);
      } else if (typeof file.text === "string") {
        // The run's own report, readable in place.
        const text = document.createElement("details");
        text.className = "auto-run-text";
        const summary = document.createElement("summary");
        summary.textContent = caption;
        const body = document.createElement("pre");
        body.textContent = file.text;
        text.append(summary, body);
        grid.appendChild(text);
      } else {
        const row = document.createElement("span");
        row.className = "auto-run-file";
        row.textContent = caption;
        grid.appendChild(row);
      }
    }
    gallery.appendChild(grid);
    if (evidence.more > 0) {
      const more = document.createElement("p");
      more.className = "auto-run-evidence-empty";
      more.textContent = t("auto.evidenceMore", "외 {{n}}개", { n: evidence.more });
      gallery.appendChild(more);
    }
  }
  // The steps that went wrong have no frame to stand behind, so they are
  // listed in words: what was tried, and what the surface answered.
  const failed = steps.filter((step) => !step.ok);
  if (failed.length > 0) {
    const list = document.createElement("ul");
    list.className = "auto-run-steps";
    for (const step of failed) {
      const item = document.createElement("li");
      item.textContent = `${stepWords(step)} — ${step.error || t("auto.stepFailed", "실패")}`;
      list.appendChild(item);
    }
    gallery.appendChild(list);
  }
  gallery.hidden = false;
  button.setAttribute("aria-expanded", "true");
}

/* The workspaces a job can be pointed at. Read from the same catalog the
 * sidebar draws, so a schedule can only name a checkout that exists. */
function paintAutoWorkspaces(selected) {
  const field = el("auto-workspace");
  field.replaceChildren();
  const seen = [];
  for (const project of projects) {
    for (const worktree of project.worktrees) {
      if (seen.includes(worktree.path)) continue;
      seen.push(worktree.path);
      const option = document.createElement("option");
      option.value = worktree.path;
      option.textContent = worktree.branch || basename(worktree.path);
      field.appendChild(option);
    }
  }
  if (selected && !seen.includes(selected)) {
    // The checkout it was pointed at is gone. Kept on the list rather than
    // silently re-pointed at another one: a job quietly moved to a different
    // repository is worse than one that shows where it used to run.
    const option = document.createElement("option");
    option.value = selected;
    option.textContent = t("auto.missingWorkspace", "{{path}} (없음)", { path: selected });
    field.appendChild(option);
  }
  field.value = selected ?? seen[0] ?? "";
}

/* The agents a job can name, from the same registry every other picker in this
 * window reads.
 *
 * The WHOLE registry, not `installedAgents()`: a schedule is written once and
 * runs for months, often against a machine the writer is not sitting at, and a
 * form that hid an agent because today's PATH does not have it would refuse a
 * record the backend is perfectly willing to store.
 *
 * The first option is absence. `agent: null` means "whatever the command
 * starts", which is this window's existing behaviour and therefore the
 * default default — the same reading `AGENT_AUTO` has on the stage. */
function paintAutoAgents(selected) {
  const field = el("auto-agent");
  field.replaceChildren();
  const fallback = document.createElement("option");
  fallback.value = "";
  fallback.textContent = t("auto.agentChoose", "에이전트 선택");
  field.appendChild(fallback);
  let known = false;
  for (const row of agentRows) {
    if (row.id === selected) known = true;
    const option = document.createElement("option");
    option.value = row.id;
    option.textContent = agentName(row.id);
    field.appendChild(option);
  }
  if (selected && !known) {
    // An id this build has never heard of — a record from a newer version, or
    // one written by hand. Kept on the list under its own id, for the reason
    // the missing workspace above is kept: editing a job's name must not
    // silently re-point it at a different agent, and the backend keeps this
    // one precisely because it was already there.
    const option = document.createElement("option");
    option.value = selected;
    option.textContent = selected;
    field.appendChild(option);
  }
  field.value = selected ?? "";
}

/* The two pickers whose options are words rather than markup.
 *
 * Built here rather than written into `index.html` with a `data-i18n` on each
 * `<option>`, because the labels are the only thing that changes and the
 * chosen value has to survive: somebody halfway through writing a schedule
 * must not have it reset by changing the language. */
function paintAutoPickers() {
  const fill = (field, entries) => {
    const held = field.value;
    field.replaceChildren();
    for (const [value, label] of entries) {
      const option = document.createElement("option");
      option.value = value;
      option.textContent = label;
      field.appendChild(option);
    }
    if (held) field.value = held;
  };
  fill(el("auto-cadence"), CADENCES.map((cadence) => [cadence.id, cadence.label()]));
  fill(
    el("auto-day"),
    Array.from({ length: WEEKDAY_COUNT }, (_, at) => [String(at), weekdayName(at)]),
  );
}

function paintAutoSchedule() {
  const cadence = el("auto-cadence").value;
  // Hourly has no hour to pick and custom has no clock at all.
  el("auto-time-field").hidden = cadence === "custom";
  el("auto-day-field").hidden = cadence !== "weekly";
  el("auto-custom-field").hidden = cadence !== "custom";
  el("auto-when").textContent = describeSchedule();
}

/* The schedule in words, so a cron expression is not the only thing on
 * screen saying what will happen. */
function describeSchedule() {
  const cadence = el("auto-cadence").value;
  const time = el("auto-time").value || "00:00";
  if (cadence === "custom") {
    const cron = el("auto-custom").value.trim();
    return cron ? t("auto.whenCron", "cron: {{cron}}", { cron }) : "";
  }
  if (cadence === "hourly") {
    return t("auto.whenHourly", "매시간 {{minute}}분", { minute: time.split(":")[1] ?? "00" });
  }
  if (cadence === "weekdays") return t("auto.whenWeekdays", "평일 {{time}}", { time });
  if (cadence === "weekly") {
    return t("auto.whenWeekly", "매주 {{day}} {{time}}", {
      day: weekdayName(Number(el("auto-day").value)),
      time,
    });
  }
  return t("auto.whenDaily", "매일 {{time}}", { time });
}

function newAutomationDraft(template = null) {
  const base = {
    id: crypto.randomUUID(),
    name: "",
    enabled: true,
    workspace: activeWorktreePath ?? "",
    workspace_mode: "existing",
    base_branch: null,
    agent: agentRows[0]?.id ?? null,
    prompt: "",
    command: "",
    reuse_session: false,
    close_when_done: false,
    evidence: "prompt",
    schedule: { cadence: "daily", time: "09:00", day_of_week: 1, custom: "" },
    precheck: "",
    precheck_timeout_seconds: 60,
    missed_run_grace_minutes: 60,
    last_run_at: null,
  };
  return template ? { ...base, ...template, schedule: { ...template.schedule } } : base;
}

function editAutomation(row, template = null) {
  autoDraft = row
    ? { ...row, schedule: { ...row.schedule } }
    : newAutomationDraft(template);
  paintAutoPickers();
  el("auto-name").value = autoDraft.name;
  el("auto-mode").value = autoDraft.workspace_mode ?? "existing";
  el("auto-base").value = autoDraft.base_branch ?? "";
  el("auto-reuse").checked = autoDraft.reuse_session;
  el("auto-close-done").checked = autoDraft.close_when_done === true;
  el("auto-evidence").value = autoDraft.evidence ?? "prompt";
  paintAutoMode();
  el("auto-command").value = autoDraft.command;
  paintAutoAgents(autoDraft.agent ?? null);
  el("auto-prompt").value = autoDraft.prompt;
  el("auto-precheck").value = autoDraft.precheck;
  el("auto-precheck-timeout").value = String(autoDraft.precheck_timeout_seconds);
  el("auto-grace").value = String(autoDraft.missed_run_grace_minutes);
  paintPrecheckTimeout();
  el("auto-cadence").value = autoDraft.schedule.cadence;
  el("auto-time").value = autoDraft.schedule.time;
  el("auto-day").value = String(autoDraft.schedule.day_of_week);
  el("auto-custom").value = autoDraft.schedule.custom;
  paintAutoWorkspaces(autoDraft.workspace);
  const stored = automations.some((held) => held.id === autoDraft.id);
  el("auto-run").hidden = !stored;
  el("auto-delete").hidden = !stored;
  el("auto-error").hidden = true;
  say(el("auto-editor-title"), () => (stored
    ? t("auto.editorEdit", "자동화 편집")
    : t("auto.editorNew", "자동화 만들기")));
  el("auto-form").hidden = false;
  paintAutoSchedule();
  const advanced = document.querySelector(".auto-advanced");
  if (advanced) advanced.open = autoDraft.command.trim() !== "";
  showModal(el("auto-editor-scrim"));
}

function closeAutoForm() {
  autoDraft = null;
  hideModal(el("auto-editor-scrim"));
  el("auto-form").hidden = true;
}

async function saveAutomation(event) {
  event.preventDefault();
  if (!autoDraft) return;
  const automation = {
    ...autoDraft,
    name: el("auto-name").value.trim(),
    workspace: el("auto-workspace").value,
    workspace_mode: el("auto-mode").value,
    // Empty means the repository's own HEAD, and the record stores that as
    // absence rather than as a magic word.
    base_branch: el("auto-base").value.trim() || null,
    // The empty option is absence, the same way the base branch above is —
    // the record has no word for "default", it simply names nobody.
    agent: el("auto-agent").value || null,
    command: el("auto-command").value.trim(),
    prompt: el("auto-prompt").value,
    precheck: el("auto-precheck").value.trim(),
    reuse_session: el("auto-mode").value === "existing" && el("auto-reuse").checked,
    precheck_timeout_seconds: Number(el("auto-precheck-timeout").value),
    missed_run_grace_minutes: Number(el("auto-grace").value),
    close_when_done: el("auto-close-done").checked,
    evidence: el("auto-evidence").value,
    schedule: {
      cadence: el("auto-cadence").value,
      time: el("auto-time").value || "00:00",
      day_of_week: Number(el("auto-day").value),
      custom: el("auto-custom").value.trim(),
    },
  };
  if (!automation.name || !automation.workspace || !automation.prompt.trim()) {
    say(el("auto-error"), () => t(
      "auto.requiredError",
      "이름, 워크스페이스와 프롬프트를 모두 입력하세요.",
    ));
    el("auto-error").hidden = false;
    return;
  }
  if (!automation.agent && !automation.command) {
    say(el("auto-error"), () => t(
      "auto.agentRequiredError",
      "실행할 에이전트를 선택하거나 사용자 명령을 입력하세요.",
    ));
    el("auto-error").hidden = false;
    return;
  }
  try {
    automations = await invoke("save_automation", { automation });
  } catch (error) {
    // Shown on the form rather than as a window-wide error: what is wrong is
    // a field the person is looking at — the schedule it cannot read, or an
    // agent it does not know, which are the two things the backend refuses.
    say(el("auto-error"), () => String(error));
    el("auto-error").hidden = false;
    return;
  }
  autoSelectedId = automation.id;
  closeAutoForm();
  paintAutomations();
  paintAutomationDetail();
  void refreshAutomationRuns();
}

/* The base-branch field only exists for the mode that reads it — a field the
 * backend would ignore is a promise the form should not make. */
function paintAutoMode() {
  const mode = el("auto-mode").value;
  el("auto-base-field").hidden = mode !== "new_per_run";
  // Reuse only means anything for the mode that keeps a checkout to reuse.
  // A field the backend would ignore is a promise the form must not make.
  el("auto-reuse-field").hidden = mode !== "existing";
  if (mode !== "existing") document.getElementById("auto-reuse").checked = false;
}

/* The timeout only bites when there is a precheck to time. Without a command
 * the select would name a limit on nothing, so it is disabled until one. */
function paintPrecheckTimeout() {
  el("auto-precheck-timeout").disabled = el("auto-precheck").value.trim() === "";
}

el("auto-mode").addEventListener("change", paintAutoMode);
el("auto-precheck").addEventListener("input", paintPrecheckTimeout);

el("auto-new").addEventListener("click", () => editAutomation(null));
el("auto-empty-new").addEventListener("click", () => editAutomation(null));
for (const button of document.querySelectorAll("[data-auto-template]")) {
  button.addEventListener("click", () => {
    const make = AUTO_TEMPLATES[button.dataset.autoTemplate];
    editAutomation(null, make ? make() : null);
  });
}
el("auto-form").addEventListener("submit", saveAutomation);
el("auto-cancel").addEventListener("click", closeAutoForm);
el("auto-editor-close").addEventListener("click", closeAutoForm);
el("auto-editor-scrim").addEventListener("click", (event) => {
  if (event.target === el("auto-editor-scrim")) closeAutoForm();
});
el("auto-cadence").addEventListener("change", paintAutoSchedule);
for (const id of ["auto-time", "auto-day", "auto-custom"]) {
  el(id).addEventListener("input", () => {
    el("auto-when").textContent = describeSchedule();
  });
}

/* Put a tab around the shell a run just opened.
 *
 * Until this existed a run was invisible: `start_automation` opened a pty and
 * handed back its id, and nothing on this side ever built a surface for it. A
 * job fired, an agent worked, and the only trace was a next-run time changing.
 *
 * The tab belongs to the RUN'S checkout, never to whatever is on screen. The
 * strip filters by the active worktree (`paneTabs`) and the board reads a
 * card's checkout off the tab holding the pane (`cardsFromPanes`), so a run
 * filed under the workspace somebody happened to leave open at 9am is a card
 * attributed to a repository the agent never touched — and a tab that appears
 * in it. For `NewPerRun` that checkout is the one this run just cut.
 *
 * Whether it takes the stage is the caller's answer, and the two doors below
 * answer differently on purpose. */
async function adoptAutomationRun(run, { focus }) {
  const term = run?.term;
  if (typeof term !== "number") return null;
  // One run is one tab. The by-hand door adopts what the command answered and
  // the event for the same run can still arrive; whichever is second finds the
  // tab already there.
  const tab = tabOfTerm(term) ?? mountRunTab(run);
  if (!focus) return tab;
  // The stage only draws the active workspace's tabs, so watching a run in
  // another checkout means going there first — the same order the board's own
  // door uses (`openPaneFromBoard`), for the same reason.
  if (tab.worktree !== activeWorktreePath && !(await activateWorktree(tab.worktree))) return tab;
  setActiveTab(tab.id);
  // A division got this shell in as a pane, and the ask was to WATCH it: the
  // keyboard follows into the run's own pane — the other half of the
  // asymmetry whose background side leaves hands exactly where they were.
  if (tab.activePane !== term && paneLeaves(tab.layout).includes(term)) {
    tab.activePane = term;
    renderPanes(tab);
    updateStage();
    keySink.focus();
  }
  return tab;
}

function mountRunTab(run) {
  // Mounted with the layout file held shut. A checkout's pane layout is
  // written from the tabs this window is holding for it, and a workspace
  // nobody has visited this session has none of them in memory yet — so
  // persisting from here would replace that file with just this one tab.
  // Nothing is lost by staying quiet: an agent tab with no resumable session
  // is excluded from the file anyway (`persistPaneLayouts`).
  const held = restoringPanes;
  restoringPanes = true;
  try {
    return mountTermTab(
      run.term,
      {
        worktree: run.root || activeWorktreePath,
        // The job's name is the tab's name — a run that reads as `터미널 7`
        // is one nobody can tell from a shell they opened themselves. An
        // unnamed job carries no name rather than an empty one, and falls
        // through to the numbered label.
        ...(run.name?.trim() && { agent: run.name.trim() }),
      },
      { focus: false },
    );
  } finally {
    restoringPanes = held;
  }
}

async function runAutomationNow(automation) {
  if (!automation) return;
  try {
    const started = await invoke("run_automation", { id: automation.id });
    setAutoOpen(false);
    refreshAutomations();
    if (started?.worktree) void refreshWorktrees();
    void adoptAutomationRun(started, { focus: true });
  } catch (error) {
    showError(error);
  }
}

el("auto-run").addEventListener("click", () => {
  if (!autoDraft) return;
  invoke("run_automation", { id: autoDraft.id })
    .then((started) => {
      setAutoOpen(false);
      refreshAutomations();
      if (started?.worktree) void refreshWorktrees();
      void adoptAutomationRun(started, { focus: true });
    })
    .catch(showError);
});

el("auto-detail-run").addEventListener("click", () => {
  void runAutomationNow(selectedAutomation());
});

el("auto-detail-edit").addEventListener("click", () => {
  const row = selectedAutomation();
  if (row) editAutomation(row);
});

el("auto-detail-toggle").addEventListener("click", async () => {
  const row = selectedAutomation();
  if (!row) return;
  const button = el("auto-detail-toggle");
  button.disabled = true;
  try {
    automations = await invoke("enable_automation", { id: row.id, enabled: !row.enabled });
    paintAutomations();
    paintAutomationDetail();
  } catch (error) {
    showError(error);
  } finally {
    button.disabled = false;
  }
});

el("auto-detail-delete").addEventListener("click", () => {
  const row = selectedAutomation();
  if (row) void requestAutomationDeletion(row).catch(showError);
});

el("auto-detail-back").addEventListener("click", () => {
  autoSelectedId = null;
  autoRuns = [];
  paintAutomations();
  paintAutomationDetail();
});

el("auto-overview-tab").addEventListener("click", () => setAutoDetailTab("overview"));
el("auto-runs-tab").addEventListener("click", () => setAutoDetailTab("runs"));

el("auto-refresh").addEventListener("click", () => void refreshAutomations());
el("auto-search").addEventListener("input", paintAutomations);
el("auto-view-list-btn")?.addEventListener("click", () => {
  autoViewMode = "list";
  paintAutomations();
});
el("auto-view-cal-btn")?.addEventListener("click", () => {
  autoViewMode = "calendar";
  paintAutomations();
});
el("auto-cal-prev")?.addEventListener("click", () => {
  autoCalMonth--;
  if (autoCalMonth < 0) {
    autoCalMonth = 11;
    autoCalYear--;
  }
  paintAutomationCalendar();
});
el("auto-cal-next")?.addEventListener("click", () => {
  autoCalMonth++;
  if (autoCalMonth > 11) {
    autoCalMonth = 0;
    autoCalYear++;
  }
  paintAutomationCalendar();
});
el("auto-cal-today")?.addEventListener("click", () => {
  const now = new Date();
  autoCalYear = now.getFullYear();
  autoCalMonth = now.getMonth();
  autoCalSelectedDate = `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
  paintAutomationCalendar();
});
window.addEventListener("focus", () => {
  if (!autoView.hidden) void refreshAutomations();
});
document.addEventListener("visibilitychange", () => {
  if (document.visibilityState === "visible" && !autoView.hidden) void refreshAutomations();
});
for (const button of document.querySelectorAll("[data-auto-filter]")) {
  button.addEventListener("click", () => {
    autoFilter = button.dataset.autoFilter;
    for (const held of document.querySelectorAll("[data-auto-filter]")) {
      held.classList.toggle("is-active", held === button);
    }
    paintAutomations();
  });
}
for (const button of document.querySelectorAll("[data-auto-sort]")) {
  button.addEventListener("click", () => {
    const field = button.dataset.autoSort;
    autoSort = autoSort?.field === field
      ? { field, direction: autoSort.direction * -1 }
      : { field, direction: field === "last" ? -1 : 1 };
    paintAutomations();
  });
}

async function deleteAutomationAndHistory(automation) {
  automations = await invoke("delete_automation", { id: automation.id });
  if (autoSelectedId === automation.id) autoSelectedId = null;
  closeAutoForm();
  autoRuns = [];
  paintAutomations();
  paintAutomationDetail();
}

async function requestAutomationDeletion(automation) {
  if (!skipDeleteAutomationConfirm) {
    const confirmed = await askConfirm({
      title: t("auto.deleteTitle", "자동화 삭제"),
      body: t(
        "auto.deleteBody",
        "{{name}} 자동화와 실행 기록을 삭제합니다. 이전 실행이 만든 워크스페이스는 삭제하지 않습니다.",
        { name: automation.name },
      ),
      remember: t("auto.deleteRemember", "다시 묻지 않기"),
      confirm: t("app.delete", "삭제"),
      deny: t("app.cancel", "취소"),
      danger: true,
      cancel: false,
    });
    if (confirmed !== true) return;
    if (askRemembered()) setAskBeforeDeletingAutomations(false);
  }
  await deleteAutomationAndHistory(automation);
}

el("auto-delete").addEventListener("click", () => {
  if (!autoDraft) return;
  void requestAutomationDeletion(autoDraft).catch(showError);
});

el("auto-close").addEventListener("click", () => setAutoOpen(false));

/* A scheduled job started while the window was open. The list's next-run
 * times just changed, a NewPerRun job put a whole new workspace in the
 * sidebar, and the shell it opened needs a surface.
 *
 * IN THE BACKGROUND, which is the whole asymmetry with the by-hand door. A
 * schedule fires on its own clock: 9am finds somebody mid-sentence in another
 * terminal, and a window that yanks the stage out from under them makes the
 * scheduler something to switch off. The tab is on the strip, the board has
 * its card, the bell rings if the agent stops to ask — every way of finding
 * the run stays open, and none of them is taken by force. */
listen("automation:started", (event) => {
  refreshAutomations();
  refreshChrome();
  if (event.payload?.worktree) void refreshWorktrees();
  void adoptAutomationRun(event.payload, { focus: false });
});

listen("automation:failed", (event) => {
  const { error } = event.payload;
  showError(error);
  refreshAutomations();
});

listen("automation:skipped", () => {
  refreshAutomations();
});

/* The agent of a run said it was done: the run's row learns it, and only the
 * job being looked at needs repainting for that. */
listen("automation:completed", (event) => {
  if (event.payload?.id === autoSelectedId) void refreshAutomationRuns();
});

// 1분 주기 자동 갱신: 자동화 화면이 열려 있을 때 일정이 지난 작업을 재계산하고 오늘 하이라이트/달력을 최신화
setInterval(() => {
  if (!autoView.hidden && document.visibilityState === "visible") {
    void refreshAutomations();
  }
}, 60000);

/* Notification and browser controls are views over the same canonical
 * settings document as theme and terminal. The OS probe is deliberately not
 * a preference write: it tests delivery without changing either category. */
let notificationPrefs = {
  enabled: true,
  agent_attention: true,
  agent_completion: true,
};

function paintNotificationPrefs() {
  const master = notificationPrefs.enabled !== false;
  el("notify-enabled").checked = master;
  el("notify-agent-attention").checked = notificationPrefs.agent_attention !== false;
  el("notify-agent-completion").checked = notificationPrefs.agent_completion !== false;
  // 마스터가 꺼지면 종류별은 만질 수 없다 — Orca NotificationsPane의
  // 관용: 죽은 스위치를 살아 있는 것처럼 두면 "껐는데 왜 꺼졌지"가 된다.
  el("notify-agent-attention").disabled = !master;
  el("notify-agent-completion").disabled = !master;
}

for (const [id, kind, field] of [
  ["notify-enabled", "enabled", "enabled"],
  ["notify-agent-attention", "agent_attention", "agent_attention"],
  ["notify-agent-completion", "agent_completion", "agent_completion"],
]) {
  el(id).addEventListener("change", (event) => {
    notificationPrefs = { ...notificationPrefs, [field]: event.target.checked };
    paintNotificationPrefs();
    void commitSetting(`notifications.${field}`, "set_notification_preference", {
      kind,
      on: event.target.checked,
    });
  });
}

el("notify-test").addEventListener("click", async () => {
  const status = el("notify-test-status");
  status.textContent = t("onboard.notifyChecking", "알림을 보내는 중…");
  try {
    const delivered = await invoke("notification_probe", {
      title: "ZeroCode",
      body: t("onboard.notifyProbeBody", "알림은 이렇게 도착합니다."),
    });
    status.textContent = delivered
      ? t("onboard.notifyDelivered", "알림이 도착했습니다.")
      : t("onboard.notifyBlocked", "시스템이 알림을 막고 있습니다.");
  } catch (error) {
    status.textContent = String(error);
    showError(error);
  }
});

function paintBrowserPrefs() {
  el("browser-home-page").value = browserPrefs.home_page ?? "";
  el("browser-search-engine").value = browserPrefs.search_engine ?? "google";
  const zoom = el("browser-default-zoom");
  const levels = browserZoomLevels();
  zoom.replaceChildren(...levels.map((level) => {
    const option = document.createElement("option");
    option.value = String(level);
    option.textContent = browserZoomPercent(level);
    return option;
  }));
  const selectedZoom = normalizeBrowserZoomLevel(browserPrefs.default_zoom_level);
  zoom.disabled = selectedZoom === null;
  if (selectedZoom !== null) zoom.value = String(selectedZoom);
  const openLinksInApp = browserPrefs.open_links_in_app === true;
  const modifierInverts = browserPrefs.open_links_in_app_modifier_inverts === true;
  el("browser-open-links-in-app").checked = openLinksInApp;
  el("browser-link-routing-modifier-inverts").checked = modifierInverts;
  const shortcut = `Shift+${primaryModifierLabel()}`;
  const withShortcut = (message) => message.replace("{{shortcut}}", shortcut);
  const routingHint = t(
    "settings.browser.openLinksInAppHint",
    "터미널, Markdown, 에디터의 http(s) 링크를 ZeroCode 내장 브라우저에서 엽니다.",
  );
  const systemHint = withShortcut(t(
    "settings.browser.systemShortcutHint",
    "{{shortcut}}는 항상 시스템 브라우저를 사용합니다.",
  ));
  el("browser-link-routing-description").textContent = modifierInverts
    ? routingHint
    : `${routingHint} ${systemHint}`;
  el("browser-link-routing-modifier-title").textContent = openLinksInApp
    ? t("settings.browser.modifierSystemTitle", "Shift를 누르면 웹 브라우저에서 열기")
    : t("settings.browser.modifierAppTitle", "Shift를 누르면 ZeroCode에서 열기");
  const modifierHint = openLinksInApp
    ? t(
      "settings.browser.modifierSystemHint",
      "링크를 열 때 {{shortcut}}를 누르면 웹 브라우저를 사용합니다.",
    )
    : t(
      "settings.browser.modifierAppHint",
      "링크를 열 때 {{shortcut}}를 누르면 ZeroCode를 사용합니다.",
    );
  el("browser-link-routing-modifier-description").textContent = withShortcut(modifierHint);
  el("browser-terminal-link-actions").checked =
    browserPrefs.terminal_link_action_popover !== false;
  el("browser-terminal-link-actions-description").textContent = t(
    "settings.browser.terminalLinkActionsHint",
    "터미널 링크를 클릭하면 열 수 있는 동작을 보여줍니다. 끄면 {{modifier}} 클릭이 필요합니다.",
  ).replace("{{modifier}}", primaryModifierLabel());
  el("browser-restore-tabs").checked = browserPrefs.restore_tabs === true;
  el("browser-preferences-status").textContent = "";
  paintBrowserUserAgents();
}

/* ---- 사이트별 사용자 에이전트 (browser-door-for-agents §2.5) ---------------
 *
 * 행 하나가 호스트 하나다. 판의 독자는 태어날 때만 정해지므로(wry) 행은
 * 「다음 판부터」 — 이미 열린 판은 제 이름을 그대로 입는다. 표는 행 단위
 * 패치(`set`/`remove`)로만 바뀐다: 옛 스냅샷을 든 창이 표 전체를 덮어
 * 이웃 창이 방금 적은 행을 지우지 못하게, 링크 라우팅과 같은 모양이다.
 * 검증은 백엔드의 것(호스트는 스킴·포트·경로 없는 소문자 이름, 에이전트는
 * 출력 가능한 ASCII 한 줄) — 창은 다듬어 보내고 거절을 그대로 보여 준다. */
function browserUserAgentRow(row) {
  const line = document.createElement("div");
  line.className = "browser-ua-row";
  line.dataset.host = row.host;
  const host = document.createElement("span");
  host.className = "browser-ua-host";
  host.textContent = row.host;
  const agent = document.createElement("span");
  agent.className = "browser-ua-agent";
  agent.textContent = row.agent;
  // 긴 이름은 한 줄로 잘려 보이니, 전체는 툴팁이 든다(네이티브 title 없음).
  agent.dataset.tip = row.agent;
  const drop = document.createElement("button");
  drop.type = "button";
  drop.className = "icon-btn browser-ua-del is-halt";
  drop.innerHTML = icon("trash");
  const word = t("settings.browser.userAgentDelete", "{{host}} 행 삭제", { host: row.host });
  drop.dataset.tip = word;
  drop.setAttribute("aria-label", word);
  drop.addEventListener("click", () => {
    void patchBrowserUserAgents({ kind: "remove", value: row.host });
  });
  line.append(host, agent, drop);
  return line;
}

function paintBrowserUserAgents() {
  const rows = Array.isArray(browserPrefs.user_agents) ? browserPrefs.user_agents : [];
  el("browser-user-agent-empty").hidden = rows.length > 0;
  el("browser-user-agent-list").replaceChildren(...rows.map(browserUserAgentRow));
}

/* 낙관적으로 그리고 한 행을 보낸다. 거절되면 `commitSetting`이 정본을 다시
 * 읽어 되돌리고, 이유는 이 표의 상태 줄에 선다. */
function patchBrowserUserAgents(patch) {
  const rows = Array.isArray(browserPrefs.user_agents) ? browserPrefs.user_agents : [];
  const host = String(patch.kind === "set" ? patch.value.host : patch.value)
    .trim()
    .toLowerCase();
  const next = patch.kind === "set"
    ? (rows.some((row) => row.host === host)
      ? rows.map((row) => (row.host === host ? { host, agent: patch.value.agent } : row))
      : [...rows, { host, agent: patch.value.agent }])
    : rows.filter((row) => row.host !== host);
  browserPrefs = { ...browserPrefs, user_agents: next };
  paintBrowserUserAgents();
  el("browser-user-agent-status").textContent = "";
  return commitSetting("browser.user_agents", "patch_browser_user_agents", { patch }, {
    onError: (error) => { el("browser-user-agent-status").textContent = String(error); },
  });
}

el("browser-user-agent-add").addEventListener("submit", (event) => {
  event.preventDefault();
  const hostField = el("browser-user-agent-host");
  const agentField = el("browser-user-agent-agent");
  const host = hostField.value.trim();
  const agent = agentField.value.trim();
  // 반쪽 행은 창을 떠나지 않는다 — 빈 쪽으로 손을 돌려보낸다.
  if (!host || !agent) {
    el("browser-user-agent-status").textContent = t(
      "settings.browser.userAgentNeedsBoth",
      "호스트와 사용자 에이전트를 둘 다 적어 주세요.",
    );
    (host ? agentField : hostField).focus();
    return;
  }
  void patchBrowserUserAgents({ kind: "set", value: { host, agent } }).then((snapshot) => {
    // 거절된 행은 필드에 남아 고칠 수 있다; 저장된 행만 폼을 비운다.
    if (snapshot === undefined) return;
    hostField.value = "";
    agentField.value = "";
    hostField.focus();
  });
});

el("browser-home-page").addEventListener("change", (event) => {
  const homePage = event.target.value.trim();
  browserPrefs = { ...browserPrefs, home_page: homePage };
  paintBrowserPrefs();
  void commitSetting("browser.home_page", "set_browser_home_page", { homePage }, {
    onError: (error) => { el("browser-preferences-status").textContent = String(error); },
  });
});

el("browser-search-engine").addEventListener("change", (event) => {
  const searchEngine = event.target.value;
  browserPrefs = { ...browserPrefs, search_engine: searchEngine };
  paintBrowserPrefs();
  void commitSetting("browser.search_engine", "set_browser_search_engine", { searchEngine });
});

el("browser-default-zoom").addEventListener("change", (event) => {
  const zoomLevel = Number(event.target.value);
  browserPrefs = { ...browserPrefs, default_zoom_level: zoomLevel };
  paintBrowserPrefs();
  void commitSetting("browser.default_zoom_level", "set_browser_default_zoom", { zoomLevel }, {
    onError: (error) => { el("browser-preferences-status").textContent = String(error); },
  });
});

function setBrowserLinkRouting(kind, value) {
  browserPrefs = { ...browserPrefs, [kind]: value };
  paintBrowserPrefs();
  void commitSetting(`browser.${kind}`, "patch_browser_link_routing", {
    patch: { kind, value },
  });
}

el("browser-open-links-in-app").addEventListener("change", (event) => {
  setBrowserLinkRouting("open_links_in_app", event.target.checked);
  // 실측 BrowserLinkRoutingSetting: 토글에 손을 댄 것 자체가 첫-클릭 질문의
  // 답이라, 스위치가 prompted를 함께 적는다.
  if (browserPrefs.open_links_in_app_prompted !== true) {
    setBrowserLinkRouting("open_links_in_app_prompted", true);
  }
});

el("browser-link-routing-modifier-inverts").addEventListener("change", (event) => {
  setBrowserLinkRouting("open_links_in_app_modifier_inverts", event.target.checked);
});

el("browser-terminal-link-actions").addEventListener("change", (event) => {
  setBrowserLinkRouting("terminal_link_action_popover", event.target.checked);
});

el("browser-restore-tabs").addEventListener("change", (event) => {
  browserPrefs = { ...browserPrefs, restore_tabs: event.target.checked };
  paintBrowserPrefs();
  void commitSetting("browser.restore_tabs", "set_browser_restore_tabs", {
    on: event.target.checked,
  });
});

/* ---- 세션 쿠키 구역 --------------------------------------------------------
 *
 * 프로필 하나가 곧 쿠키 단지 하나다. 가져오는 기계는 이미 다 있었고
 * (`cookie_sources` · `import_browser_cookies` · `askCookieSource`), 닿는
 * 길만 브라우저 도구모음 하나였다 — 브라우저를 한 번도 안 연 사람에게는
 * 없는 기능과 같았다. 원본도 같은 구역을 설정의 브라우저 페이지에 둔다
 * (`BrowserPane.tsx:276-290`). */

/** 프로필이 마지막으로 마신 자리의 이름 — 원본의 `sourceLabel`
 *  (`BrowserPane.tsx:188-190`)과 같은 모양: 소스 프로필이 있으면 괄호로. */
function cookieSourceLabel(source) {
  return source.profile ? `${source.label} (${source.profile})` : source.label;
}

/* 새 탭이 이 프로필로 열리도록 고르거나(활성), 이미 활성이면 내장 기본
 * 저장소로 되돌린다. 우리에겐 원본처럼 "기본" 카드가 따로 없어, 이 토글이
 * 그 자리로 가는 유일한 문이다. 창은 값만 바꾸고 진실은 곁파일이 든다. */
async function toggleDefaultBrowserProfile(id) {
  try {
    await invoke("set_browser_default_profile", { id });
  } catch (error) {
    showError(String(error));
    return;
  }
  await refreshBrowserProfiles();
  paintBrowserProfiles();
}

/* 프로필 하나를 잊는다 — 확인을 묻고(되돌릴 수 없다), 목록·쿠키 단지·기본
 * 표시를 시스템 손이 함께 거둔다(delete_browser_profile). */
async function deleteBrowserProfile(profile) {
  const yes = await askConfirm({
    title: t("settings.browser.profileDelete", "프로필 삭제"),
    body: t(
      "settings.browser.profileDeleteBody",
      "{{name}} 프로필과 그 쿠키 단지를 지웁니다. 되돌릴 수 없습니다.",
      { name: profile.name },
    ),
    confirm: t("settings.browser.profileDeleteConfirm", "삭제"),
    deny: t("app.cancel", "취소"),
    danger: true,
    cancel: false,
  });
  if (yes !== true) return;
  try {
    await invoke("delete_browser_profile", { id: profile.id });
  } catch (error) {
    showError(String(error));
    return;
  }
  await refreshBrowserProfiles();
  paintBrowserProfiles();
}

/* 한 프로필 카드 — 원본 `BrowserProfileRow`를 우리 계약 위에 다시 세운 것.
 * 카드 자체가 단추라 눌러 기본을 고르고(활성 배지), 스테이징이 앉아 있으면
 * "적용 대기" 칩이 선다(원본엔 없는, 스테이징 주입 계약을 눈에 보이게 한
 * 우리 쪽 이득). 오른쪽 액션은 카드의 "기본으로"와 섞이지 않는다. */
function browserProfileRow(profile) {
  const row = document.createElement("div");
  row.className = "browser-profile-row";
  const active = profile.isDefault === true;
  if (active) row.classList.add("is-active");
  // 위임된 툴팁 한 줄(네이티브 title 금지 — 게이트) + 마우스와 키보드를 한
  // 손으로 쥐는 actsAsButton(row 게이트). 카드 자체가 "새 탭 기본으로" 단추다.
  row.dataset.tip = active
    ? t("settings.browser.profileClearDefault", "다시 누르면 내장 기본 저장소로 돌아갑니다")
    : t("settings.browser.profileMakeDefault", "새 브라우저 탭을 이 프로필로 엽니다");
  actsAsButton(row, () => void toggleDefaultBrowserProfile(active ? null : profile.id));

  const identity = document.createElement("div");
  identity.className = "browser-profile-identity";
  const head = document.createElement("div");
  head.className = "browser-profile-head";
  const name = document.createElement("p");
  name.className = "browser-profile-name";
  name.textContent = profile.name;
  head.append(name);
  if (active) {
    const badge = document.createElement("span");
    badge.className = "browser-profile-badge";
    badge.textContent = t("settings.browser.profileActive", "활성");
    head.append(badge);
  }
  if (profile.staged === true) {
    const chip = document.createElement("span");
    chip.className = "browser-profile-chip";
    chip.textContent = t("settings.browser.profilePending", "적용 대기");
    chip.dataset.tip = t("browser.importReopenHint", "이 프로필의 브라우저를 다시 열면 적용됩니다");
    head.append(chip);
  }
  const note = document.createElement("p");
  note.className = "browser-profile-note";
  note.textContent = profile.source
    ? t("settings.browser.profileFrom", "마지막으로 {{label}}에서 가져왔습니다", {
      label: cookieSourceLabel(profile.source),
    })
    : t("settings.browser.profileNever", "아직 가져온 쿠키가 없습니다");
  identity.append(head, note);

  const acts = document.createElement("div");
  acts.className = "browser-profile-acts";
  // 액션은 카드의 "기본으로" 클릭으로 새면 안 된다.
  acts.addEventListener("click", (event) => event.stopPropagation());

  const bring = document.createElement("button");
  bring.type = "button";
  bring.className = "btn browser-profile-import";
  const importing = browserImportingId === profile.id;
  bring.disabled = importing;
  if (importing) {
    bring.textContent = t("settings.browser.profileImporting", "가져오는 중…");
  } else {
    bring.textContent = profile.source
      ? t("settings.browser.profileReimport", "다시 가져오기")
      : t("settings.browser.profileImport", "가져오기");
  }
  bring.addEventListener("click", (event) => {
    void askCookieImport(event.currentTarget.getBoundingClientRect(), profile.id);
  });

  const drop = document.createElement("button");
  drop.type = "button";
  drop.className = "icon-btn browser-profile-del is-halt";
  drop.innerHTML = icon("trash");
  drop.dataset.tip = t("settings.browser.profileDelete", "프로필 삭제");
  drop.setAttribute("aria-label", t("settings.browser.profileDelete", "프로필 삭제"));
  drop.addEventListener("click", () => void deleteBrowserProfile(profile));

  acts.append(bring, drop);
  row.append(identity, acts);
  return row;
}

function paintBrowserProfiles() {
  el("browser-profile-empty").hidden = browserProfiles.length > 0;
  el("browser-profile-list").replaceChildren(...browserProfiles.map(browserProfileRow));
}

el("browser-profile-new").addEventListener("click", (event) => {
  askNewBrowserProfile(event.currentTarget.getBoundingClientRect(), null);
});

/* Open or close the page, on the same terms 작업 uses.
 *
 * Both are stops in the titlebar walk, both close the other, and both record
 * the return to the workspace terminal when they close — Orca's
 * `recordViewVisit("automations")` and the plain-terminal exception with it.
 * Written out rather than shared with `setTaskOpen`: the two differ in what
 * they refresh, and folding them together would put a branch inside every
 * line of it. */
function setAutoOpen(on) {
  if (on) {
    closePalette();
    crossingPages = true;
    setTaskOpen(false);
    setSettingsOpen(false);
    setSpaceOpen(false);
    crossingPages = false;
    refreshAutomations();
    recordNavVisit("automations");
    void askForTour("automations");
  } else if (!autoView.hidden && !crossingPages && activeWorktreePath) {
    recordNavVisit(activeWorktreePath);
  }
  autoView.hidden = !on;
  if (!on) closeAutoForm();
  paintRouteWidth();
  paintNavCurrent();
  if (!on && termFloat.hidden) keySink.focus();
}

el("settings-open-automations").addEventListener("click", () => setAutoOpen(true));

/* ---- 아티팩트 보존 (t-2720) ----------------------------------------------
 *
 * 숫자 하나: 스토어가 행과 복사본을 며칠 두는가. 0은 「원장과 같음」이고
 * 기본이다 — 설정을 열어 본 적 없는 사람은 런이 남는 만큼 보고서도 남는다.
 * 경계는 백엔드의 표(`artifacts_retention_spec`)에서 오고, 이 파일은 그 값을
 * 필드에 옮겨 적을 뿐이다. */
let artifactsRetentionDays = 0;
let artifactsRetentionSpec = null;

function paintArtifactsRetention() {
  const field = el("artifacts-retention-days");
  field.value = String(artifactsRetentionDays);
  if (artifactsRetentionSpec) {
    field.min = String(artifactsRetentionSpec.min);
    field.max = String(artifactsRetentionSpec.max);
    field.step = String(artifactsRetentionSpec.step);
  }
  const hint = el("artifacts-retention-days-hint");
  say(hint, () => artifactsRetentionDays === 0
    ? t("settings.artifacts.retentionHintLedger", "0이면 원장의 보존 기간을 따릅니다. 보고서는 그 런이 남는 만큼 남습니다.")
    : t("settings.artifacts.retentionHintDays", "{{n}}일이 지난 아티팩트는 카탈로그와 복사본이 함께 정리됩니다.", {
      n: artifactsRetentionDays,
    }));
}

function commitArtifactsRetention() {
  const field = el("artifacts-retention-days");
  const asked = Number(field.value.trim());
  if (!Number.isFinite(asked)) {
    paintArtifactsRetention();
    return;
  }
  const spec = artifactsRetentionSpec ?? { min: 0, max: asked, step: 1 };
  const next = Math.min(spec.max, Math.max(spec.min, Math.round(asked)));
  if (next === artifactsRetentionDays) {
    paintArtifactsRetention();
    return;
  }
  artifactsRetentionDays = next;
  paintArtifactsRetention();
  void commitSetting("artifacts_retention_days", "set_artifacts_retention_days", { days: next });
}

el("artifacts-retention-days").addEventListener("blur", commitArtifactsRetention);
el("artifacts-retention-days").addEventListener("keydown", (event) => {
  if (event.key !== "Enter") return;
  event.preventDefault();
  commitArtifactsRetention();
});
el("settings-open-artifacts").addEventListener("click", () => {
  setSettingsOpen(false);
  leavePagesForStage();
  openArtifacts({ fresh: true });
});

/* Which sources are put away. Empty until the boot report says otherwise —
 * a window that shows a hidden tab for one frame has not hidden it. */
const hiddenTaskSources = new Set();
const SUPPORTED_TASK_SOURCES = new Set(["jira", "github", "gitlab", "linear"]);

function paintTaskSources() {
  for (const name of TASK_SOURCES) {
    const supported = SUPPORTED_TASK_SOURCES.has(name);
    const away = hiddenTaskSources.has(name);
    const tab = el(`task-tab-${name}`);
    tab.hidden = !supported || away;
    tab.dataset.tip = TASK_SOURCE_NAMES[name];
    tab.setAttribute("aria-label", TASK_SOURCE_NAMES[name]);
    const control = el(`show-source-${name}`);
    control.checked = supported && !away;
    control.disabled = !supported;
  }
  // 사이드바 프로젝트 행의 지름길도 같은 기준을 입는다(1-g48) — 탭에는 없는
  // 문이 행에만 서 있으면 같은 글리프가 겹으로 보인다.
  for (const button of document.querySelectorAll(".project-source-btn")) {
    const source = button.dataset.source;
    button.hidden = !SUPPORTED_TASK_SOURCES.has(source) || hiddenTaskSources.has(source);
  }
  const showing = TASK_SOURCES.filter(
    (name) => SUPPORTED_TASK_SOURCES.has(name) && !hiddenTaskSources.has(name),
  );
  if (showing.length === 0) {
    hiddenTaskSources.delete("jira");
    showing.push("jira");
    el("task-tab-jira").hidden = false;
    el("show-source-jira").checked = true;
  }
  el("task-none").hidden = true;
  // Unconditional, not only when the current source was just hidden: this is
  // also what puts `aria-selected` on all four at boot. A `role="tab"` that
  // never says whether it is the selected one is a tab strip with no state
  // for anybody reading it through the accessibility tree.
  //
  // 물러서는 것이지 고르는 것이 아니다 — 고른 소스가 돌아오면 사람은 그 자리로
  // 돌아온다.
  setTaskSource(
    showing.includes(chosenTaskSource) ? chosenTaskSource : showing[0],
    { chosen: false },
  );
  paintDefaultTaskSource();
}

function setTaskSourceHidden(name, away) {
  const visible = TASK_SOURCES.filter(
    (source) => SUPPORTED_TASK_SOURCES.has(source) && !hiddenTaskSources.has(source),
  );
  if (away && visible.length === 1 && visible[0] === name) {
    el(`show-source-${name}`).checked = true;
    return;
  }
  if (away) hiddenTaskSources.add(name);
  else hiddenTaskSources.delete(name);
  paintTaskSources();
  // Patch only the row the person touched. A second window may have changed a
  // different source since this one last read the list; replacing the whole
  // stale list would silently put that other change back.
  void commitSetting("hidden_task_sources", "set_task_source_visibility", {
    source: name,
    visible: !away,
  });
}

for (const name of TASK_SOURCES) {
  el(`show-source-${name}`).addEventListener("change", (event) => {
    if (!SUPPORTED_TASK_SOURCES.has(name)) {
      event.target.checked = false;
      return;
    }
    setTaskSourceHidden(name, !event.target.checked);
  });
}

el("jira-hide").addEventListener("click", () => setTaskSourceHidden("jira", true));

function paintDefaultTaskSource() {
  const picker = el("task-default-source");
  if (!picker) return;
  for (const option of picker.options) {
    option.disabled = hiddenTaskSources.has(option.value);
  }
  picker.value = defaultTaskSource;
}

el("task-default-source").addEventListener("change", (event) => {
  defaultTaskSource = event.target.value;
  paintDefaultTaskSource();
  void commitSetting("default_task_source", "set_default_task_source", {
    source: defaultTaskSource,
  });
});

/* ---- 작업판의 GitHub(1-g50) ----
 *
 * 문은 gh다(Orca TaskPage 실측): 미설치·미로그인의 빈 상태는 할 일을
 * 이름하고, 로그인 명령은 복사 한 번으로 손에 쥐어진다 — 이 창은 토큰을
 * 들지 않는다. 목록은 이 창의 재료로 선다: 컴포저가 이미 읽는 gh work
 * item(이슈·PR)을 활성 프로젝트에서 묻는다 — Orca의 화면은 아직 포팅하지
 * 않은 GitHub Projects를 그린다(의도적 발산, 원장 1-g50). */
const GITHUB_SURFACES = [
  "github-loading",
  "github-empty",
  "github-nosource",
  "github-quiet",
  "github-none",
  "github-list",
];

/* 도구 단이 설 수 있는 얼굴들 — 목록의 것이지 진단의 것이 아니다. 「기다리는
 * 중」이 드는 것은 필수다: 새로 고침 한 번에 단이 사라지면 방금 누른 단추가
 * 화면에서 없어진다. */
const GITHUB_TOOL_SURFACES = new Set(["github-loading", "github-none", "github-list"]);

function showGithubOnly(id, said) {
  showOneSurface(GITHUB_SURFACES, id, said);
  el("github-tools").hidden = !GITHUB_TOOL_SURFACES.has(id);
  // 필 줄은 도구 단에 딸린 것이다: 단이 물러난 얼굴 위에 「무엇을 좁혔다」만
  // 남으면, 그 말을 받을 목록이 화면에 없다.
  paintGithubFilters();
}

/* 종류 셋 중 이 창이 아는 둘. Orca에는 「프로젝트」가 하나 더 서지만 우리에게
 * 그 기능이 없어 세그먼트에도 서지 않는다(의도적 발산, 1-g77a). */
let taskGithubKind = "issue";

/* 지금 검색칸에 든 문장이 어느 칩의 것인가, 아니면 아무의 것도 아닌가.
 * 켜진 칩은 이 하나로만 그려진다 — 칩이 쓴 문장을 사람이 한 자라도 고치면
 * 그 목록은 더 이상 그 칩의 것이 아니다. */
let taskGithubChip = null;
let taskGithubAsking = null;
let taskGithubTimer = null;

/* 종류마다의 프리셋 칩(실측 신판: 이슈는 열기·나에게 할당됨, PR은 열기·내
 * 것·리뷰 필요). 값은 `gh.rs`가 아는 낱말이고 — 모르는 낱말은 거절된다 —
 * 그 낱말이 무슨 질의인지는 이 창이 알지 못한다. 물어서 받아 적는다. */
const GITHUB_KIND_CHIPS = {
  issue: [
    ["open", () => t("task.github.chipOpen", "열기")],
    ["assigned", () => t("task.github.chipAssigned", "나에게 할당됨")],
  ],
  pr: [
    ["open", () => t("task.github.chipOpen", "열기")],
    ["mine", () => t("task.github.chipMine", "내 것")],
    ["review", () => t("task.github.chipReview", "리뷰 필요")],
  ],
};

/* 고른 필터 — **값**이지 문법이 아니다(1-g70b).
 *
 * Orca에서는 검색 문자열 하나가 유일한 진실이고 필터 팝오버가 그 문자열에
 * 한정자를 써 넣는다. 이 창은 그럴 수 없다: GitHub 검색 문법의 사본은
 * `gh.rs` 하나뿐이라는 것이 이 저장소의 게이트이고
 * (`every_github_read_goes_through_one_door`), 창이 한정자를 지으면 사람이
 * 고른 필터와 실제로 나간 질문이 서로 다른 날이 온다. 그래서 여기 남는 것은
 * 「무엇을 골랐나」뿐이고, 그것을 질문으로 옮기는 손은 `search_plan` 하나다.
 * 검색칸도 마찬가지로 친 그대로 나가는 자유 문장일 뿐이다. */
let taskGithubFilters = { state: null, draft: false, author: null, assignee: null };

/* 이 판이 아는 상태 셋. 창은 값만 들고, `state:` 한정자는 gh.rs가 짓는다. */
const GITHUB_FILTER_STATES = ["open", "closed", "merged"];

/* 750ms. Orca가 GitHub 검색에만 박아 둔 값이고, Jira 쪽의
 * `TASK_SEARCH_DEBOUNCE_MS = 300`이 아니다(실측 1-g70) — 이쪽의 한 번은
 * `gh`를 띄워 GitHub에 나가는 요청이라 더 오래 기다린다. */
const GITHUB_SEARCH_DEBOUNCE_MS = 750;

/* 지금 물어야 할 질의, 없으면 빈 문자열. 다듬는 것은 양끝 공백뿐 — 한정자
 * 문법의 주인은 GitHub이고(`gh --search`), 여기서 거르면 GitHub이 답했을
 * 질의를 이 창이 대신 거절하게 된다. */
function taskGithubQuery() {
  return el("task-gh-search").value.trim();
}

/* 도구 단은 한 자리에서만 그려진다: 무엇을 물으러 가든 이 손이 그 질문을
 * 말한다.
 *
 * 신판 실측의 계약은 「칸에 든 문장이 곧 지금의 질문」이다. 그래서 켜진 칩은
 * 그 문장을 써 넣은 칩 하나뿐이고, 사람이 칸을 고치는 순간 아무 칩도 켜지지
 * 않는다 — 화면이 두 가지로 대답하지 않게. */
function paintGithubTools() {
  selectSegment("github-kinds", taskGithubKind);
  el("github-presets").replaceChildren(
    ...(GITHUB_KIND_CHIPS[taskGithubKind] ?? []).map(([value, produce]) => {
      const chip = document.createElement("button");
      chip.type = "button";
      chip.className = "task-chip";
      chip.dataset.ghPreset = value;
      const on = value === taskGithubChip;
      chip.classList.toggle("is-active", on);
      chip.setAttribute("aria-pressed", String(on));
      say(chip, produce);
      return chip;
    }),
  );
  el("task-gh-search-clear").hidden = taskGithubQuery() === "";
  paintGhProjectsCombo();
}

/* ---- 프로젝트 범위(1-g77d, 실측의 [모든 프로젝트 ▾]) ----
 *
 * 빈 선택은 「전부」다 — 보드 필터의 그 의미론. 목록의 원천은 카탈로그
 * (`projects`)이고, 콤보가 고른 집합이 조회의 폭이 된다. */
let ghProjectsChosen = new Set();

function ghProjectHomes() {
  return projects.map((project) => ({ path: project.path, name: project.name }));
}

function ghProjectSpan(homes) {
  return ghProjectsChosen.size
    ? homes.filter((home) => ghProjectsChosen.has(home.path))
    : homes;
}

/* 콤보의 낱말: 없음/전부/하나의 이름/「이름 외 N개」 — 실측의 그 세 얼굴. */
function paintGhProjectsCombo() {
  const combo = el("gh-projects-combo");
  const homes = ghProjectHomes();
  combo.disabled = homes.length === 0;
  say(combo, () => {
    if (homes.length === 0) return t("task.github.noProjects", "프로젝트 없음");
    const span = ghProjectSpan(homes);
    if (span.length === homes.length) return t("task.github.allProjects", "모든 프로젝트");
    if (span.length === 1) return span[0].name;
    return t("task.github.someProjects", "{{name}} 외 {{count}}개", {
      name: span[0].name,
      count: span.length - 1,
    });
  });
  paintTaskContext();
}

/* 실측의 컨텍스트 라벨: 제공자 · 호스트 · 대상. 이 창의 작업 판은 로컬
 * 체크아웃 위에 선다 — 원격 호스트의 판이 서는 날 이 낱말이 갈라진다. */
function paintTaskContext() {
  const label = el("task-context");
  label.hidden = false;
  say(label, () => {
    let target;
    if (taskSource === "github") {
      const homes = ghProjectHomes();
      const span = ghProjectSpan(homes);
      target = span.length === 1
        ? span[0].name
        : t("task.github.projectCount", "프로젝트 {{count}}개", { count: span.length });
    } else if (taskSource === "jira") {
      const sites = Array.isArray(jiraStatus?.sites) ? jiraStatus.sites : [];
      const selected = sites.find((site) => site.id === jiraStatus?.selected_site_id);
      target = selected ? jiraSiteName(selected) : t("task.context.currentAccount", "현재 계정");
    } else if (taskSource === "linear") {
      target = linearStatus?.connection?.organization_name
        ?? t("task.context.currentAccount", "현재 계정");
    } else {
      target = projects.find((project) => project.path === activeProjectPath)?.name
        ?? basename(activeWorktreePath ?? activeProjectPath ?? "");
    }
    return [TASK_SOURCE_NAMES[taskSource], t("task.context.localHost", "로컬"), target]
      .filter(Boolean)
      .join(" · ");
  });
}

function closeGhProjectsPop() {
  closing(el("gh-projects-pop"));
  el("gh-projects-combo").setAttribute("aria-expanded", "false");
}

function taskFilterSection(host, labelText, rows) {
  if (rows.length === 0) return;
  const label = document.createElement("div");
  label.className = "note-pop-label";
  label.textContent = labelText;
  host.appendChild(label);
  for (const row of rows) {
    const line = document.createElement("label");
    line.className = "settings-check";
    const box = document.createElement("input");
    box.type = "checkbox";
    box.checked = row.on;
    box.onchange = row.flip;
    const words = document.createElement("span");
    words.className = "settings-label";
    words.textContent = row.said;
    line.append(box, words);
    host.appendChild(line);
  }
}

/* 팝오버는 열릴 때마다 다시 지어진다(보드 필터의 그 판). 전부를 해제하면
 * 「전부」로 돌아온다 — 아무것도 안 보는 화면은 고를 수 있는 것이 아니다. */
function openGhProjectsPop() {
  const host = el("gh-projects-body");
  host.replaceChildren();
  const homes = ghProjectHomes();
  taskFilterSection(
    host,
    t("task.github.projects", "프로젝트"),
    homes.map((home) => ({
      said: home.name,
      on: ghProjectsChosen.size === 0 || ghProjectsChosen.has(home.path),
      flip: () => {
        const chosen = ghProjectsChosen.size
          ? ghProjectsChosen
          : new Set(homes.map((one) => one.path));
        if (chosen.has(home.path)) chosen.delete(home.path);
        else chosen.add(home.path);
        // 전부를 고른 것은 빈 선택으로 접는다 — 명시적 전부는 그날 이후
        // 카탈로그에 온 프로젝트를 폭 밖에 세우고, 빈 「전부」는 품는다.
        // (빈 집합은 이미 「전부」다 — ghProjectSpan의 그 계약.)
        ghProjectsChosen = chosen.size === homes.length ? new Set() : chosen;
        openGhProjectsPop();
        paintGhProjectsCombo();
        void loadTaskGithub();
      },
    })),
  );
  const pop = el("gh-projects-pop");
  showing(pop);
  el("gh-projects-combo").setAttribute("aria-expanded", "true");
  placeUnder(pop, el("gh-projects-combo"));
}

el("gh-projects-combo").addEventListener("click", () => {
  if (overlayClosed(el("gh-projects-pop"))) openGhProjectsPop();
  else closeGhProjectsPop();
});

dismissable(el("gh-projects-pop"), closeGhProjectsPop, ".task-gh-projects");

/* 진단이 얼굴을 고른다: gh가 없으면 설치를, 로그인이 없으면 로그인 명령을,
 * 둘 다 서 있으면 목록을. */
async function paintGithubPanel() {
  showGithubOnly("github-loading");
  await ensureGithubStatus();
  if (githubStatus.availability === "missing") {
    showGithubOnly("github-empty");
    el("github-install").hidden = false;
    el("github-login").hidden = true;
    say(el("github-empty-body"), () =>
      t(
        "task.github.missing",
        "GitHub CLI(gh)가 설치되어 있지 않거나 PATH에 없습니다. cli.github.com에서 설치한 뒤 로그인하세요.",
      ));
    return;
  }
  const signedIn = githubStatus.accounts.some((account) => account.standing === "connected");
  if (!signedIn) {
    showGithubOnly("github-empty");
    el("github-install").hidden = true;
    el("github-login").hidden = false;
    say(el("github-empty-body"), () =>
      t(
        "task.github.signedOut",
        "gh로 GitHub에 로그인되어 있지 않습니다. 로그인을 누르면 앱 터미널에서 gh가 안내합니다.",
      ));
    return;
  }
  // 로그인은 섰지만 물어볼 저장소가 없는 사정(실측 상태, 1-g77a). 진단이 이미
  // 아는 사실이므로 새로 묻지 않는다: 원격이 GitHub이 아니거나
  // (`not_repository`) 저장소를 확인하지 못한 것(`unavailable`)이고, 둘 다
  // 사람이 할 일은 목록을 고치는 것이 아니라 다시 찾아보는 것이다.
  // 프로젝트가 열려 있어도 원격이 GitHub이 아니면 물어볼 목록은 없다: 이 조건에
  // 「프로젝트 없음」을 AND로 덧붙인 44f5e812는 「다시 시도」 뒤 소스 없음 안내를
  // 목록 로드로 흘려보냈다(창 하네스 「undetected source offers retry」).
  if (["not_repository", "unavailable"].includes(githubStatus.repo_standing)) {
    showGithubOnly("github-nosource");
    return;
  }
  await loadTaskGithub();
}

/* 「다시 확인」과 「다시 시도」가 하는 일은 하나다: PATH까지 다시 읽는 진단
 * 한 번, 그리고 그 답이 고른 얼굴. 두 벌로 두면 한쪽만 고쳐지는 날이 온다. */
async function recheckGithubPanel() {
  showGithubOnly("github-loading");
  await refreshGithubIntegration(null, true);
  await paintGithubPanel();
}

/* 컴포저의 그 질문을 이 판이 다시 묻는다 — 물은 것과 온 답의 궤가 다르면
 * 답은 버린다(`loadWorktreeGithub`의 관용 그대로). */
async function loadTaskGithub() {
  paintGithubTools();
  // 폭은 콤보가 정한다(1-g77d): 카탈로그의 프로젝트들 중 고른 집합, 빈
  // 선택은 전부다. 카탈로그가 비어 있으면 지금까지의 그 안내가 선다.
  const span = ghProjectSpan(ghProjectHomes());
  const project = span[0]?.path ?? activeWorktreePath;
  if (!project) {
    showGithubOnly(
      "github-quiet",
      t("task.github.noProject", "프로젝트를 열면 그 저장소의 이슈와 PR이 여기 섭니다."),
    );
    return;
  }
  const query = taskGithubQuery();
  // 계약이 바뀐 자리다(1-g77a). 칩은 이제 제 질의를 **검색칸에** 써 넣으므로
  // 이 판이 보내는 preset은 언제나 빈 값이다. 프리셋과 친 질의가 함께 나가면
  // `gh::search_plan`이 둘을 이어 붙이고(`parts.join(" ")`), 이슈로 좁히는
  // 프리셋 위에 PR 쪽 한정자가 서면 그런 항목은 GitHub에 없어 답이 언제나
  // 빈다. 인자 자체는 남는다 — 컴포저는 아직 이름으로 프리셋을 고르고, 그
  // 이름을 질의로 옮기는 손은 `gh.rs` 하나뿐이다.
  // A blank box still belongs to the selected kind. Without this seed the
  // backend's recent-work fallback asks both issues and pull requests while
  // the Issues button remains selected.
  const preset = query === "" ? (taskGithubKind === "pr" ? "prs" : "issues") : "";
  // 좁힌 것이 무엇이든 — 친 글자든 고른 필터든 — 빈 답의 뜻이 달라진다. 답이
  // 왔을 때의 화면이 아니라 **물었을 때**의 상태로 판단해야 하므로 여기서 잰다.
  const narrowed = query !== "" || activeGithubFilters().length > 0;
  // 물은 것의 이름에는 친 것도, 고른 필터도, 그리고 종류도 든다 — 그것만 다른
  // 두 질문이 한 이름을 가지면 늦게 온 옛 답이 새 목록 위에 그려진다.
  const asking = [
    span.map((home) => home.path).join(","),
    project, taskGithubKind, preset, query, githubFilterSignature(),
  ].join(" ");
  taskGithubAsking = asking;
  showGithubOnly("github-loading");
  let rows;
  try {
    // 필터는 값 그대로 건넌다. 없는 것은 아무것도 좁히지 않는 것이므로 기본값이
    // 곧 예전의 그 질문이다 — 이 인자를 모르는 백엔드도 같은 답을 준다.
    rows = await invoke("github_work_items", {
      project,
      projects: span.map((home) => home.path),
      preset,
      query,
      filters: taskGithubFilters,
    });
  } catch (error) {
    if (taskGithubAsking !== asking) return;
    const failure = failureOf(error);
    showGithubOnly(
      "github-quiet",
      failure.kind === "missing" || !failure.message
        ? t("worktree.ghMissing", "GitHub CLI(gh)로 읽습니다 — 설치하고 로그인해 주세요")
        : failure.message,
    );
    return;
  }
  if (taskGithubAsking !== asking) return;
  const items = Array.isArray(rows) ? rows : [];
  if (items.length === 0) {
    sayGithubNone(narrowed);
    showGithubOnly("github-none");
    return;
  }
  showGithubOnly("github-list");
  el("github-list").replaceChildren(githubTableHead(), ...items.map(taskGithubRow));
}

/* 빈 답에는 두 가지가 있고, 사람이 할 일이 서로 다르다: 스스로 좁힌 것이 있으면
 * — 친 질의든 고른 필터든 — 고치거나 지울 것이 있고, 프리셋뿐이라면 고칠 것이
 * 없다. 무엇이 물었는지는 답이 온 그때의 상태가 말한다 — 화면에 선 목록의
 * 것이지 지금 칸에 든 글자의 것이 아니므로. `say`로 말하는 것은 언어가 바뀔 때
 * 열쇠의 문장이 이 판단을 덮어쓰지 않게 하기 위해서다(실측 문구: "No matching
 * GitHub work"). */
function sayGithubNone(narrowed) {
  say(el("github-none-title"), () => (narrowed
    ? t("task.github.noMatch", "일치하는 GitHub 작업이 없습니다")
    : t("task.github.none", "항목이 없습니다")));
  say(el("github-none-body"), () => (narrowed
    ? t("task.github.noMatchHint", "쿼리를 변경하거나 지우세요.")
    : t("task.github.noneBody", "이 프로젝트에서 이 조건에 맞는 열린 항목이 없습니다.")));
}

/* 다음 질의를 예약한다. 한 글자마다 `gh`를 띄우지 않기 위한 것이고, 커밋을
 * 서두르는 손(Enter·칩·지우기)은 예약을 먼저 취소한 뒤 스스로 부른다. */
function scheduleTaskGithub() {
  clearTimeout(taskGithubTimer);
  taskGithubTimer = setTimeout(() => void loadTaskGithub(), GITHUB_SEARCH_DEBOUNCE_MS);
}

function loadTaskGithubNow() {
  clearTimeout(taskGithubTimer);
  void loadTaskGithub();
}

/* 표의 머리 — 신판 실측의 다섯 낱말이 다섯 칸의 이름이다. 목록과 함께
 * 그려지는 것은, 목록이 없을 때(빈 판·미감지 판) 이름만 선 표가 아무것도
 * 이름하지 않기 때문이다. */
function githubTableHead() {
  const head = document.createElement("div");
  head.className = "task-gh-table-head";
  for (const said of [
    () => t("task.github.colId", "ID"),
    () => t("task.github.colTitle", "제목"),
    () => t("task.github.colAssignee", "담당자"),
    () => t("task.github.colState", "상태"),
    () => t("task.github.colUpdated", "업데이트됨"),
  ]) {
    const cell = document.createElement("span");
    say(cell, said);
    head.appendChild(cell);
  }
  return head;
}

/* 한 행 — 신판 실측의 표: ID | 제목·맥락 | 담당자 | 상태 | 업데이트됨.
 * 행 전체는 여전히 하나의 손이다("이 항목 열기" — Jira 행과 같은 문). */
function taskGithubRow(item) {
  // A `<button>`, like the composer's gh rows: the keyboard reaches it
  // without `actsAsButton`, the helper that exists for `div` rows.
  const choice = document.createElement("button");
  choice.type = "button";
  choice.className = "wt-gh-row task-gh-row";
  choice.dataset.number = String(item.number);
  choice.dataset.kind = item.kind;

  const id = document.createElement("span");
  id.className = "task-gh-cell-id";
  id.append(workItemMark(item), workItemNumber(item));

  // 제목 위, 맥락 아래 — 맥락은 컴포저의 행이 쓰는 그 조각들이다. 초안은
  // 여기 없다: 상태 칸이 그 낱말을 이미 말한다.
  const titled = document.createElement("span");
  titled.className = "task-gh-cell-title";
  const title = document.createElement("span");
  title.className = "wt-gh-title";
  title.textContent = item.title;
  titled.appendChild(title);
  const badges = workItemBadges(item);
  // 폭 있는 목록에서만 행이 제 프로젝트를 입에 올린다(단일 조회는 침묵 —
  // 백엔드의 그 계약). 이름은 카탈로그가 알고, 모르면 경로의 끝 조각이다.
  if (item.project) {
    const home = document.createElement("span");
    home.className = "wt-gh-frag task-gh-home";
    home.textContent = projects.find((one) => one.path === item.project)?.name
      ?? item.project.split("/").at(-1);
    badges.unshift(home);
  }
  if (badges.length > 0) {
    const context = document.createElement("span");
    context.className = "task-gh-context";
    context.append(...badges);
    titled.appendChild(context);
  }

  // 담당자는 API가 준 순서의 핸들들 — 없음은 숨길 부재가 아니라 대시로 찍는
  // 사실이다(빈 칸은 "아직 안 왔다"로도 읽힌다).
  const assignee = document.createElement("span");
  assignee.className = "task-gh-cell-assignee";
  assignee.textContent = item.assignees?.length ? item.assignees.join(", ") : "—";

  const state = document.createElement("span");
  state.className = `wt-gh-pill task-gh-cell-state is-${item.state}`;
  say(state, () => ghItemStateWord(item.state));

  const when = document.createElement("span");
  when.className = "task-gh-cell-when";
  say(when, () => ghAgoSentence(item.updated_at));

  choice.append(id, titled, assignee, state, when);
  choice.addEventListener("click", () => openGithubItem(item));
  return choice;
}

/* ---- GitHub 항목 다이얼로그(1-g54) ---------------------------------------
 * Orca의 GitHubItemDialog 1단: 행 하나가 상세를 열고, 본문과 대화가 이 창의
 * 마크다운 렌더러(구성상 안전 — 스크립트는 글자로 남는다)로 그려지며,
 * 코멘트와 닫기/다시 열기가 gh의 손으로 나간다. 작업 시작은 목록 행이 쓰던
 * 그 문(startWorktreeFromWorkItem)이다. */
const ghItemScrim = el("gh-item-scrim");
let ghItem = null;
let ghItemAsking = null;
let ghItemBusy = false;
/* 어느 판이 서 있나(t-2733) — 카드가 열릴 때마다 대화로 돌아온다. */
let ghItemTab = "talk";
/* 파일 탭의 기억: 경로 → 이 PR의 그 파일 diff. 한 카드 안에서는 같은 파일을
 * 두 번 묻지 않는다(전환에 재요청 0). 카드가 열릴 때 비운다 — 다른 PR의
 * diff를 이 번호 아래 보여 주는 실수는 여기서 막는다. */
let ghFileDiffs = new Map();
let ghFileChosen = null;
/* 리뷰어 벤치 — 관리를 누른 손에게만 읽힌다(null이면 접혀 있다). */
let ghItemBench = null;

function ghItemStateWord(state) {
  if (state === "merged") return t("task.github.stateMerged", "병합됨");
  if (state === "closed") return t("task.github.stateClosed", "닫힘");
  if (state === "draft") return t("task.github.stateDraft", "초안");
  return t("task.github.stateOpen", "열림");
}

function ghAgoSentence(iso) {
  const at = Date.parse(iso ?? "");
  if (!at) return "";
  return t("task.github.agoSentence", "{{when}} 전", { when: agoWord(at, Date.now()) });
}

function openGithubItem(item) {
  ghItem = {
    ...item,
    // 폭 있는 목록의 행은 제 프로젝트를 안다 — 활성 체크아웃이 아니라 그
    // 저장소에 물어야 42번이 그 42번이다.
    project: item.project ?? activeWorktreePath,
  };
  showModal(ghItemScrim);
  // 목록이 이미 아는 사실로 먼저 서고, 상세가 도착하면 그 위를 덮는다.
  el("gh-item-number").textContent = `#${item.number}`;
  el("gh-item-title").textContent = item.title ?? "";
  const pill = el("gh-item-state");
  pill.className = `wt-gh-pill gh-item-state is-${item.state}`;
  say(pill, () => ghItemStateWord(ghItem?.state ?? item.state));
  ghItem.state = item.state;
  el("gh-item-mark")
    .querySelector("use")
    ?.setAttribute("href", item.kind === "pr" ? "#i-pr" : "#i-tasks");
  el("gh-item-meta").textContent = "";
  el("gh-item-body").replaceChildren();
  el("gh-item-comments-head").textContent = "";
  el("gh-item-comments").replaceChildren();
  el("gh-item-error").hidden = true;
  el("gh-item-state-act").hidden = true;
  el("gh-item-merge").hidden = true;
  ghItemTab = "talk";
  ghFileDiffs = new Map();
  ghFileChosen = null;
  ghItemBench = null;
  el("gh-item-tabs").hidden = true;
  el("gh-item-reviewers").hidden = true;
  el("gh-item-reviewer-bench").hidden = true;
  el("gh-item-threads-head").hidden = true;
  el("gh-item-threads").replaceChildren();
  el("gh-item-files").replaceChildren();
  el("gh-item-file-diff").replaceChildren();
  el("gh-item-checks").replaceChildren();
  el("gh-item-hosted-acts").hidden = true;
  paintGhItemTabs();
  void readGithubItem();
}

function closeGithubItem() {
  if (ghItemBusy) return;
  hideModal(ghItemScrim);
  ghItem = null;
  ghItemAsking = null;
  el("gh-item-comment-field").value = "";
}

async function readGithubItem() {
  const at = ghItem;
  if (!at) return;
  const asking = [at.project, at.kind, at.number].join(" ");
  ghItemAsking = asking;
  el("gh-item-loading").hidden = false;
  el("gh-item-error").hidden = true;
  let detail;
  try {
    detail = await invoke("github_work_item_detail", {
      project: at.project,
      kind: at.kind,
      number: at.number,
    });
  } catch (error) {
    if (ghItemAsking !== asking) return;
    el("gh-item-loading").hidden = true;
    const failure = failureOf(error);
    el("gh-item-error").hidden = false;
    say(el("gh-item-error"), () =>
      failure.message || t("task.github.detailFailed", "항목을 읽지 못했습니다"));
    return;
  }
  if (ghItemAsking !== asking) return;
  el("gh-item-loading").hidden = true;
  paintGithubItem(detail);
}

/* 한 목소리의 카드. 두 판(GitHub·GitLab)이 같은 얼굴을 쓴다 — 누가·언제 한 줄,
 * 그 아래 이 창의 마크다운(구성상 안전: 스크립트는 글자로 남는다). 판마다 다른
 * 것은 옷의 접두사와, 머리에 더 붙는 표 하나뿐이라 그 둘만 받는다.
 *
 * `said`가 함수인 것은 `say`의 규칙 때문이다: 언어가 바뀌면 이 줄은 스스로 다시
 * 쓰여야 하고, 그때 다시 물을 수 있는 것은 글자가 아니라 손이다. */
function itemVoiceCard(prefix, said, body, badge = null) {
  const card = document.createElement("article");
  card.className = `${prefix}-item-comment`;
  const head = document.createElement("p");
  head.className = `${prefix}-item-comment-head`;
  // 말은 제 칸에 든다. `say`가 쓰는 것은 그 칸의 textContent이므로, 표를 머리에
  // 바로 붙이면 언어가 바뀌는 순간 그 표가 지워진다.
  const who = document.createElement("span");
  say(who, said);
  head.appendChild(who);
  if (badge) head.appendChild(badge);
  const written = document.createElement("div");
  written.className = `${prefix}-item-comment-body`;
  if (body.trim()) paintMarkdown(written, body);
  card.append(head, written);
  return card;
}

function ghCommentNode(comment) {
  return itemVoiceCard("gh", () => [
    comment.author ?? t("task.github.ghost", "(알 수 없는 손)"),
    ghAgoSentence(comment.written_at),
  ].filter(Boolean).join(" · "), comment.body);
}

function paintGithubItem(detail) {
  if (!ghItem) return;
  ghItem.state = detail.state;
  ghItem.url = detail.url || ghItem.url;
  // 통째로 든다 — 병합 사다리가 읽는 것은 GitHub의 낱말 그대로다.
  ghItem.told = detail;
  el("gh-item-merge").hidden = !(detail.kind === "pr" && detail.state === "open");
  el("gh-item-number").textContent = `#${detail.number}`;
  el("gh-item-title").textContent = detail.title;
  const pill = el("gh-item-state");
  pill.className = `wt-gh-pill gh-item-state is-${detail.state}`;
  say(pill, () => ghItemStateWord(ghItem?.state ?? detail.state));
  say(el("gh-item-meta"), () => [detail.author, ghAgoSentence(detail.opened_at)]
    .filter(Boolean).join(" · "));
  const body = el("gh-item-body");
  body.replaceChildren();
  if (detail.body.trim()) {
    paintMarkdown(body, detail.body);
  } else {
    const quiet = document.createElement("p");
    quiet.className = "gh-item-quiet";
    say(quiet, () => t("task.github.noBody", "본문이 없습니다."));
    body.appendChild(quiet);
  }
  say(el("gh-item-comments-head"), () =>
    t("task.github.comments", "코멘트 {{count}}", { count: detail.comments.length }));
  el("gh-item-comments").replaceChildren(...detail.comments.map(ghCommentNode));
  paintGhItemDepth(detail);
  // 닫힌 것은 다시 열 수 있고, 열린 것은 닫을 수 있다 — 병합된 PR만 둘 다
  // 아니다. 권한의 최종 판정은 gh의 몫이고, 여기는 문만 세운다.
  const act = el("gh-item-state-act");
  const closed = detail.state === "closed";
  act.hidden = detail.state === "merged";
  act.dataset.wantOpen = String(closed);
  say(act, () => {
    if (closed) return t("task.github.reopen", "다시 열기");
    return detail.kind === "pr"
      ? t("task.github.closePr", "PR 닫기")
      : t("task.github.closeIssue", "이슈 닫기");
  });
}

/* ---- PR의 깊이(t-2733): 탭 셋, 파일, 검사, 리뷰어, 스레드 -----------------
 *
 * Orca TaskPage의 PR 구역을 다이얼로그 안에 접은 것이다. 백엔드가 detail 뒤에
 * 덧붙인 넷(files·checks·reviewers·conversation)을 그대로 그리고, 파일 diff
 * 하나만 늦게 묻는다 — 파일 단위로, 한 카드 안에서 한 번씩. */
function paintGhItemDepth(detail) {
  const pr = detail.kind === "pr";
  el("gh-item-tabs").hidden = !pr;
  if (!pr) {
    ghItemTab = "talk";
    paintGhItemTabs();
    return;
  }
  const files = detail.files ?? [];
  const checks = detail.checks?.rows ?? [];
  say(el("gh-item-tab-files"), () => (files.length
    ? t("task.github.tabFilesCount", "파일 {{count}}", { count: files.length })
    : t("task.github.tabFiles", "파일")));
  say(el("gh-item-tab-checks"), () => (checks.length
    ? t("task.github.tabChecksCount", "검사 {{count}}", { count: checks.length })
    : t("task.github.tabChecks", "검사")));
  paintGhReviewers(detail.reviewers ?? []);
  paintGhThreads(detail.conversation ?? []);
  paintGhFiles(files);
  paintGhChecks(detail, checks);
  paintGhItemTabs();
}

/* 세 판 중 하나가 선다. 컴포저와 푸터는 탭 밖이다(GitLab 카드의 그 해부). */
function paintGhItemTabs() {
  for (const button of el("gh-item-tabs").querySelectorAll("[data-gh-tab]")) {
    const here = button.dataset.ghTab === ghItemTab;
    button.classList.toggle("is-active", here);
    button.setAttribute("aria-selected", here ? "true" : "false");
  }
  el("gh-item-pane-talk").hidden = ghItemTab !== "talk";
  el("gh-item-pane-files").hidden = ghItemTab !== "files";
  el("gh-item-pane-checks").hidden = ghItemTab !== "checks";
}

el("gh-item-tabs").addEventListener("click", (event) => {
  const button = event.target.closest("[data-gh-tab]");
  if (!button || button.dataset.ghTab === ghItemTab) return;
  ghItemTab = button.dataset.ghTab;
  paintGhItemTabs();
  // 파일 판에 처음 서면 첫 파일을 연다 — 빈 diff 칸을 두고 누르라 하지 않는다.
  if (ghItemTab === "files" && ghFileChosen === null) {
    const first = ghItem?.told?.files?.[0];
    if (first) void showGhFileDiff(first.path);
  }
});

/* GitHub의 status 낱말을 변경 목록이 쓰는 그 글자로 — 장식 색도 같은 표에서. */
function ghFileLetter(status) {
  const letters = {
    added: "A",
    removed: "D",
    modified: "M",
    renamed: "R",
    copied: "C",
    changed: "M",
  };
  return letters[status] ?? "";
}

/* 파일 목록: 글자·경로·±. 고른 행이 diff 칸의 주인이다. */
function paintGhFiles(files) {
  const host = el("gh-item-files");
  if (files.length === 0) {
    const quiet = document.createElement("p");
    quiet.className = "gh-item-quiet";
    say(quiet, () => t("task.github.noFiles", "바뀐 파일이 없습니다."));
    host.replaceChildren(quiet);
    el("gh-item-file-diff").replaceChildren();
    return;
  }
  host.replaceChildren(...files.map((file) => {
    // A real button, so it is reachable by keyboard on its own — no
    // `actsAsButton` and no `row` to wire.
    const door = document.createElement("button");
    door.type = "button";
    door.className = "gh-item-file-row";
    door.classList.toggle("is-chosen", file.path === ghFileChosen);
    door.dataset.path = file.path;
    const letter = ghFileLetter(file.status);
    const badge = document.createElement("span");
    badge.className = "badge";
    badge.textContent = letter;
    badge.dataset.git = letter ? gitDecorationOf(letter) : "";
    const name = document.createElement("span");
    name.className = "history-file-path";
    name.innerHTML = '<span class="change-file"></span><span class="change-dir"></span>';
    const faces = pathFaces(file.path);
    name.querySelector(".change-file").textContent = faces.file;
    name.querySelector(".change-dir").textContent = faces.dir;
    const counts = document.createElement("span");
    counts.className = "gl-item-file-counts";
    const grown = document.createElement("span");
    grown.className = "gl-item-file-plus";
    grown.textContent = `+${file.additions}`;
    const gone = document.createElement("span");
    gone.className = "gl-item-file-minus";
    gone.textContent = `-${file.deletions}`;
    counts.append(grown, gone);
    door.dataset.tip = file.previous_path ? `${file.previous_path} → ${file.path}` : file.path;
    door.append(badge, name, counts);
    door.addEventListener("click", () => void showGhFileDiff(file.path));
    return door;
  }));
}

/* 한 파일의 diff — 이 카드에서 처음이면 한 번 묻고, 그 뒤로는 기억에서.
 * 그리는 손은 diff 탭의 그 손(paintDiffLines·diffLimitCard)이다. */
async function showGhFileDiff(path) {
  const at = ghItem;
  if (!at) return;
  ghFileChosen = path;
  for (const row of el("gh-item-files").querySelectorAll(".gh-item-file-row")) {
    row.classList.toggle("is-chosen", row.dataset.path === path);
  }
  let view = ghFileDiffs.get(path);
  if (!view) {
    const asking = ghItemAsking;
    try {
      view = await invoke("github_pr_file_diff", {
        project: at.project,
        number: at.number,
        path,
      });
    } catch (error) {
      if (ghItemAsking !== asking) return;
      const failure = failureOf(error);
      el("gh-item-error").hidden = false;
      say(el("gh-item-error"), () =>
        failure.message || t("task.github.diffFailed", "diff를 읽지 못했습니다"));
      return;
    }
    if (ghItemAsking !== asking) return;
    ghFileDiffs.set(path, view);
  }
  if (ghFileChosen !== path) return;
  paintGhFileDiff(path, view);
}

function paintGhFileDiff(path, view) {
  const host = el("gh-item-file-diff");
  host.replaceChildren();
  if (view.limit) {
    host.appendChild(diffLimitCard(path, view.limit));
    return;
  }
  paintDiffLines(host, path, view.lines ?? [], () => paintGhFileDiff(path, view));
}

/* 리뷰어 칩: login과 그 사람이 선 자리. ×는 그 login 하나를 remove로 보낸다 —
 * 남는 배열을 다시 보내는 것이 아니라, 변형 없이 그 이름 그대로. */
function ghReviewerStateWord(state) {
  const said = {
    requested: t("task.github.reviewerRequested", "요청됨"),
    approved: t("task.github.reviewerApproved", "승인"),
    changes_requested: t("task.github.reviewerChanges", "변경 요청"),
    commented: t("task.github.reviewerCommented", "코멘트"),
  };
  return said[state] ?? state;
}

function paintGhReviewers(reviewers) {
  const card = el("gh-item-reviewers");
  card.hidden = false;
  const chips = el("gh-item-reviewer-chips");
  if (reviewers.length === 0) {
    const quiet = document.createElement("span");
    quiet.className = "gh-item-quiet";
    say(quiet, () => t("task.github.noReviewers", "리뷰어가 없습니다."));
    chips.replaceChildren(quiet);
  } else {
    chips.replaceChildren(...reviewers.map((reviewer) => {
      const chip = document.createElement("span");
      chip.className = "gl-item-reviewer gh-item-reviewer";
      chip.dataset.state = reviewer.state;
      const name = document.createElement("span");
      name.textContent = reviewer.login;
      const stand = document.createElement("span");
      stand.className = "gh-item-reviewer-state";
      say(stand, () => ghReviewerStateWord(reviewer.state));
      const drop = document.createElement("button");
      drop.type = "button";
      drop.className = "gl-item-reviewer-drop gh-item-reviewer-drop";
      drop.textContent = "×";
      drop.setAttribute("aria-label",
        t("task.github.removeReviewer", "리뷰어 {{name}} 제거", { name: reviewer.login }));
      drop.addEventListener("click", () => void setGhReviewers([reviewer.login], true));
      chip.append(name, stand, drop);
      return chip;
    }));
  }
  paintGhReviewerBench();
}

/* 벤치: 이미 앉은 사람은 후보에서 빠진다. */
function paintGhReviewerBench() {
  const bench = el("gh-item-reviewer-bench");
  bench.hidden = ghItemBench === null;
  if (ghItemBench === null) return;
  const seated = new Set((ghItem?.told?.reviewers ?? []).map((one) => one.login));
  const pick = el("gh-item-reviewer-pick");
  pick.replaceChildren(...[
    (() => {
      const door = document.createElement("option");
      door.value = "";
      say(door, () => t("task.github.addReviewer", "추가"));
      return door;
    })(),
    ...ghItemBench
      .filter((login) => !seated.has(login))
      .map((login) => {
        const row = document.createElement("option");
        row.value = login;
        row.textContent = login;
        return row;
      }),
  ]);
}

async function setGhReviewers(logins, remove) {
  if (!ghItem || ghItemBusy) return;
  ghItemBusy = true;
  try {
    await invoke("github_set_reviewers", {
      project: ghItem.project,
      number: ghItem.number,
      logins,
      remove,
    });
    ghItemBusy = false;
    await readGithubItem();
  } catch (error) {
    ghItemBusy = false;
    const failure = failureOf(error);
    el("gh-item-error").hidden = false;
    say(el("gh-item-error"), () =>
      failure.message || t("task.github.reviewersFailed", "리뷰어를 바꾸지 못했습니다"));
  }
}

el("gh-item-reviewers-manage").addEventListener("click", async () => {
  if (!ghItem) return;
  if (ghItemBench !== null) {
    ghItemBench = null;
    paintGhReviewerBench();
    return;
  }
  const asking = ghItemAsking;
  try {
    const bench = await invoke("github_assignable_users", { project: ghItem.project });
    if (ghItemAsking !== asking) return;
    ghItemBench = bench ?? [];
  } catch (error) {
    if (ghItemAsking !== asking) return;
    const failure = failureOf(error);
    el("gh-item-error").hidden = false;
    say(el("gh-item-error"), () =>
      failure.message || t("task.github.reviewersFailed", "리뷰어를 바꾸지 못했습니다"));
    return;
  }
  paintGhReviewerBench();
});

el("gh-item-reviewer-put").addEventListener("click", () => {
  const login = el("gh-item-reviewer-pick").value;
  if (!login) return;
  void setGhReviewers([login], false);
});

/* 리뷰 스레드: 뿌리 한 장, 답글은 접혀 들고 수를 말한다. 목소리 카드는
 * 코멘트가 쓰는 그 얼굴(itemVoiceCard)이다. */
function ghThreadNode(thread) {
  const host = document.createElement("div");
  host.className = "gh-thread";
  let badge = null;
  if (thread.path) {
    badge = document.createElement("span");
    badge.className = "gh-thread-at";
    badge.textContent = thread.line ? `${thread.path}:${thread.line}` : thread.path;
  }
  const root = itemVoiceCard("gh", () => [
    thread.author ?? t("task.github.ghost", "(알 수 없는 손)"),
    ghAgoSentence(thread.at),
  ].filter(Boolean).join(" · "), thread.body, badge);
  host.appendChild(root);
  const replies = thread.replies ?? [];
  if (replies.length > 0) {
    const fold = document.createElement("button");
    fold.type = "button";
    fold.className = "btn btn--ghost gh-thread-fold";
    fold.setAttribute("aria-expanded", "false");
    say(fold, () => t("task.github.threadReplies", "답글 {{count}}", { count: replies.length }));
    const held = document.createElement("div");
    held.className = "gh-thread-replies";
    held.hidden = true;
    held.replaceChildren(...replies.map((reply) => itemVoiceCard("gh", () => [
      reply.author ?? t("task.github.ghost", "(알 수 없는 손)"),
      ghAgoSentence(reply.at),
    ].filter(Boolean).join(" · "), reply.body)));
    fold.addEventListener("click", () => {
      held.hidden = !held.hidden;
      fold.setAttribute("aria-expanded", held.hidden ? "false" : "true");
    });
    host.append(fold, held);
  }
  return host;
}

function paintGhThreads(threads) {
  const head = el("gh-item-threads-head");
  head.hidden = threads.length === 0;
  say(head, () => t("task.github.threads", "리뷰 스레드 {{count}}", { count: threads.length }));
  el("gh-item-threads").replaceChildren(...threads.map(ghThreadNode));
}

/* 검사 탭: 패널의 그 행(checkRow)을 접힘 없이, 그리고 닫힘/병합 액션 행. */
function paintGhChecks(detail, checks) {
  const host = el("gh-item-checks");
  if (checks.length === 0) {
    const quiet = document.createElement("p");
    quiet.className = "gh-item-quiet";
    say(quiet, () => t("task.github.noChecks", "검사가 없습니다."));
    host.replaceChildren(quiet);
  } else {
    host.replaceChildren(...checks.map((check, index) => checkRow(check, index, { expandable: false })));
  }
  paintHostedActs(el("gh-item-hosted-acts"), {
    state: detail.state,
    url: detail.url,
    onReopen: async () => {
      if (!ghItem) return;
      try {
        await invoke("github_set_work_item_open", {
          project: ghItem.project,
          kind: "pr",
          number: ghItem.number,
          open: true,
        });
        await readGithubItem();
        void loadTaskGithub();
      } catch (error) {
        const failure = failureOf(error);
        el("gh-item-error").hidden = false;
        say(el("gh-item-error"), () =>
          failure.message || t("task.github.stateFailed", "상태를 바꾸지 못했습니다"));
      }
    },
  });
}

/* 실측 presentGitHubPRMergeState(github-pr-merge-methods-90pnCYeJ.js:289)의
 * 사다리 그대로 — 자동 병합 액션과 checks 요약, merge queue 분기만 이 조각
 * 밖이다(상세가 그 필드를 아직 싣지 않는다 — 없는 데이터의 분기는 dead
 * code다). GitHub GraphQL 철자를 그대로 읽고, 번역은 이 표에만 산다. */
function ghMergeGate(told) {
  if (told?.state === "merged") return { said: t("task.github.gate.merged", "병합됨"), can: false };
  if (told?.state === "closed") return { said: t("task.github.gate.closed", "닫힘"), can: false };
  if (told?.state === "draft") return { said: t("task.github.gate.draft", "초안"), can: false };
  if (told?.review_decision === "REVIEW_REQUIRED") {
    return { said: t("task.github.gate.approval", "승인 필요"), can: false };
  }
  if (told?.review_decision === "CHANGES_REQUESTED") {
    return { said: t("task.github.gate.changes", "변경 요청됨"), can: false };
  }
  const mergeable = told?.mergeable;
  const standing = told?.merge_state_status;
  if (mergeable === undefined && standing === undefined) {
    return { said: t("task.github.gate.unknown", "병합 상태를 알 수 없음"), can: false };
  }
  if (mergeable === "CONFLICTING" || standing === "DIRTY") {
    return { said: t("task.github.gate.conflicts", "충돌"), can: false };
  }
  if (standing === "BEHIND") return { said: t("task.github.gate.behind", "뒤처짐"), can: false };
  if (standing === "BLOCKED") return { said: t("task.github.gate.blocked", "차단됨"), can: false };
  if (mergeable === "MERGEABLE" || standing === "CLEAN") {
    return { said: t("task.github.gate.able", "병합 가능"), can: true };
  }
  return { said: t("task.github.gate.checking", "확인 중"), can: false };
}

function closeGhMergeMenu() {
  closing(el("gh-merge-menu"));
}

el("gh-item-merge").addEventListener("click", () => {
  if (!ghItem || ghItemBusy) return;
  const pop = el("gh-merge-menu");
  const host = el("gh-merge-menu-body");
  host.replaceChildren();
  const gate = ghMergeGate(ghItem.told);
  const word = document.createElement("p");
  word.className = "gl-merge-gate";
  word.dataset.tone = gate.can ? "ready" : "halt";
  say(word, () => gate.said);
  host.appendChild(word);
  // 실측 순서: 스쿼시가 먼저다(GITHUB_PR_MERGE_METHODS), 라벨도 GitHub의 것.
  menuRow(host, {
    said: t("task.github.mergeSquash", "스쿼시 후 병합"),
    disabled: !gate.can,
    close: closeGhMergeMenu,
    run: () => void mergeGhItem("squash"),
  });
  menuRow(host, {
    said: t("task.github.mergeCommit", "병합 커밋 생성"),
    disabled: !gate.can,
    close: closeGhMergeMenu,
    run: () => void mergeGhItem("merge"),
  });
  menuRow(host, {
    said: t("task.github.mergeRebase", "리베이스 후 병합"),
    disabled: !gate.can,
    close: closeGhMergeMenu,
    run: () => void mergeGhItem("rebase"),
  });
  showing(pop);
  const at = el("gh-item-merge").getBoundingClientRect();
  const wide = pop.offsetWidth;
  const tall = pop.offsetHeight;
  pop.style.left = `${Math.max(8, Math.min(at.left, window.innerWidth - wide - 8))}px`;
  pop.style.top = `${Math.max(8, Math.min(at.bottom + 6, window.innerHeight - tall - 8))}px`;
});

dismissable(el("gh-merge-menu"), closeGhMergeMenu);

async function mergeGhItem(method) {
  if (!ghItem || ghItemBusy) return;
  ghItemBusy = true;
  try {
    await invoke("github_merge_pr", {
      project: ghItem.project,
      number: ghItem.number,
      method,
    });
    ghItemBusy = false;
    await readGithubItem();
  } catch (error) {
    ghItemBusy = false;
    const failure = failureOf(error);
    el("gh-item-error").hidden = false;
    say(el("gh-item-error"), () =>
      failure.message || t("task.github.mergeFailed", "병합하지 못했습니다"));
  }
}

el("gh-item-shut").addEventListener("click", closeGithubItem);
ghItemScrim.addEventListener("mousedown", (event) => {
  if (event.target === ghItemScrim) closeGithubItem();
});
el("gh-item-copy-link").addEventListener("click", () => {
  if (ghItem) void clipboardText.write(ghItem.url);
});
el("gh-item-open-web").addEventListener("click", () => {
  // 링크 라우팅 정책 그대로 — 문서 링크와 같은 주인이 정한 문으로.
  if (ghItem) routeHttpLink(ghItem.url, null);
});
el("gh-item-start").addEventListener("click", () => {
  if (!ghItem) return;
  const item = ghItem;
  ghItemBusy = false;
  closeGithubItem();
  void startWorktreeFromProviderItem(item, "github");
});
el("gh-item-comment-send").addEventListener("click", async () => {
  if (!ghItem || ghItemBusy) return;
  const field = el("gh-item-comment-field");
  if (!field.value.trim()) return;
  ghItemBusy = true;
  const send = el("gh-item-comment-send");
  send.disabled = true;
  try {
    await invoke("github_comment_work_item", {
      project: ghItem.project,
      kind: ghItem.kind,
      number: ghItem.number,
      body: field.value,
    });
    field.value = "";
    await readGithubItem();
  } catch (error) {
    const failure = failureOf(error);
    el("gh-item-error").hidden = false;
    say(el("gh-item-error"), () =>
      failure.message || t("task.github.commentFailed", "코멘트를 보내지 못했습니다"));
  } finally {
    ghItemBusy = false;
    send.disabled = false;
  }
});
el("gh-item-state-act").addEventListener("click", async () => {
  if (!ghItem || ghItemBusy) return;
  ghItemBusy = true;
  const act = el("gh-item-state-act");
  act.disabled = true;
  try {
    await invoke("github_set_work_item_open", {
      project: ghItem.project,
      kind: ghItem.kind,
      number: ghItem.number,
      open: act.dataset.wantOpen === "true",
    });
    await readGithubItem();
    // 목록의 그 행도 새 상태를 입어야 한다.
    void loadTaskGithub();
  } catch (error) {
    const failure = failureOf(error);
    el("gh-item-error").hidden = false;
    say(el("gh-item-error"), () =>
      failure.message || t("task.github.stateFailed", "상태를 바꾸지 못했습니다"));
  } finally {
    ghItemBusy = false;
    act.disabled = false;
  }
});

/* ---- 작업판 GitHub의 도구 단(1-g70·1-g77a) ---- */

/* 칩 하나가 검색칸에 제 질의를 써 넣는다(실측 신판: 칩 클릭 = 검색 문자열
 * 교체). 문장을 짓는 것은 이 창이 아니다 — `gh.rs`의 그 표 하나에 물어 받아
 * 적는다. 그래서 사람은 지금 무엇을 물었는지 칸에서 읽고, 그 문장을 고쳐 다시
 * 물을 수도 있다. */
async function pickGithubChip(chip) {
  let composed;
  try {
    composed = await invoke("github_preset_query", { kind: taskGithubKind, preset: chip });
  } catch (error) {
    // 창과 Rust의 어휘가 어긋난 때뿐이다(둘은 함께 배포된다). 삼키지는
    // 않는다: 아무 일도 일어나지 않는 단추가 화면에 남는 것이 더 나쁘다.
    showError(failureOf(error).message || String(error));
    return;
  }
  taskGithubChip = chip;
  el("task-gh-search").value = composed;
  loadTaskGithubNow();
}

/* 종류를 옮기면 그 종류의 첫 칩이 선다.
 *
 * 칸에 든 문장이 곧 화면의 목록이므로, 옛 종류의 질의를 남겨 두면 세그먼트와
 * 목록이 서로 다른 말을 한다 — 그리고 신판의 칸은 언제나 지금의 질의를
 * 보여 준다(실측). GitLab 판이 보기를 옮길 때 칩을 첫 칩으로 되돌리는 것과
 * 같은 규칙이다. */
el("github-kinds").addEventListener("click", (event) => {
  const button = event.target.closest(".segment-btn");
  if (!button || button.dataset.value === taskGithubKind) return;
  taskGithubKind = button.dataset.value;
  paintGithubTools();
  void pickGithubChip(GITHUB_KIND_CHIPS[taskGithubKind][0][0]);
});

el("github-presets").addEventListener("click", (event) => {
  const chip = event.target.closest("[data-gh-preset]");
  if (!chip) return;
  void pickGithubChip(chip.dataset.ghPreset);
});

el("task-gh-search").addEventListener("input", () => {
  // 칸을 고치는 순간 그 문장은 더 이상 어느 칩의 것도 아니다.
  taskGithubChip = null;
  paintGithubTools();
  scheduleTaskGithub();
});

/* Enter는 기다림을 건너뛴다 — 다만 조합 중인 글자는 아직 질의가 아니다.
 * 한국어 한 음절은 여러 번의 keydown으로 만들어지고 그 사이의 Enter는 IME가
 * 음절을 확정하는 키다. `isComposing`만으로는 부족해 이 창의 다른 문들과 같은
 * 셋을 본다(`Process`와 레거시 229). */
el("task-gh-search").addEventListener("keydown", (event) => {
  if (event.isComposing || event.key === "Process" || event.keyCode === 229) return;
  if (event.key !== "Enter") return;
  event.preventDefault();
  loadTaskGithubNow();
});

/* 지우기는 좁힌 것을 모두 놓는다: 빈 칸의 질문은 이 저장소의 최근 작업
 * 전부다(프리셋 없음 = 두 목록, `gh::search_plan`). */
el("task-gh-search-clear").addEventListener("click", () => {
  el("task-gh-search").value = "";
  taskGithubChip = null;
  el("task-gh-search").focus();
  loadTaskGithubNow();
});

/* 새로 고침은 같은 질문을 일부러 다시 묻는다 — 예약된 타이핑만 취소하고 곧장
 * 나간다(GitLab 판의 그 손과 같다). */
el("github-refresh").addEventListener("click", () => loadTaskGithubNow());

/* 웹으로 나가는 두 손짓의 한 문. 주소를 아는 것은 저장소를 아는 쪽뿐이라
 * `gh.rs`가 짓고, 이 창은 프로젝트마다 한 번만 묻고 기억한다 — 단추 한 번에
 * `gh` 하나를 띄우는 것은 목록을 다시 읽는 값과 같다. */
let taskGithubWeb = null;

async function githubWebUrls() {
  const project = activeWorktreePath;
  if (!project) return null;
  if (taskGithubWeb?.project !== project) {
    taskGithubWeb = { project, urls: await invoke("github_web_urls", { project }) };
  }
  return taskGithubWeb.urls;
}

async function openGithubWeb(pick) {
  let urls;
  try {
    urls = await githubWebUrls();
  } catch (error) {
    showError(failureOf(error).message || String(error));
    return;
  }
  const url = urls && pick(urls);
  if (url) openExternal(url);
}

el("github-new-issue").addEventListener("click", () => void openGithubWeb((urls) => urls.new_issue));

/* 「GitHub에서 열기」는 지금 보고 있는 그 목록으로 간다 — 종류 세그먼트가
 * 고른 그쪽이다. */
el("github-open-web").addEventListener("click", () =>
  void openGithubWeb((urls) => (taskGithubKind === "pr" ? urls.pulls : urls.issues)));

/* ---- 작업판 GitHub의 필터(1-g70b) ----
 *
 * Orca의 `PRFilterDropdowns`(w-72) 1단: 상태 넷과 초안 토글. 고른 것은 필로
 * 서고, 필을 누르면 그 하나만 걷힌다.
 *
 * 이쪽과 저쪽이 갈리는 자리는 하나다. Orca에서 고른 필터는 검색 문자열에
 * 한정자로 합류하지만, 이 창은 값으로만 든다 — 이유는 `taskGithubFilters`
 * 곁에 적어 두었다. */

/* 지금 켜져 있는 필터들, 필과 배지가 함께 읽는 한 목록. 이름은 사람이 고른
 * 그 낱말이다 — 필이 「상태」라고만 말하면 무엇을 좁혔는지는 팝오버를 열어야
 * 알 수 있다. */
function activeGithubFilters() {
  const chosen = [];
  if (GITHUB_FILTER_STATES.includes(taskGithubFilters.state)) {
    chosen.push({ id: "state", name: ghItemStateWord(taskGithubFilters.state) });
  }
  if (taskGithubFilters.draft) {
    chosen.push({ id: "draft", name: t("task.github.filterDraft", "초안만") });
  }
  if (taskGithubFilters.author) {
    chosen.push({ id: "author", name: t("task.github.filterAuthorPill",
      "작성자: {{name}}", { name: taskGithubFilters.author }) });
  }
  if (taskGithubFilters.assignee) {
    chosen.push({ id: "assignee", name: t("task.github.filterAssigneePill",
      "담당자: {{name}}", { name: taskGithubFilters.assignee }) });
  }
  return chosen;
}

/* 물은 것의 이름에 실리는 필터의 몫. 값이지 문장이 아니므로 언어가 바뀌어도
 * 같은 질문은 같은 이름을 갖는다. */
function githubFilterSignature() {
  return [
    taskGithubFilters.state ?? "",
    taskGithubFilters.draft ? "draft" : "",
    taskGithubFilters.author ?? "",
    taskGithubFilters.assignee ?? "",
  ].join(":");
}

/* 배지와 필 줄은 한 손이 그린다 — 「몇 개를 좁혔나」와 「무엇을 좁혔나」가 두
 * 곳에서 계산되면 반드시 어긋난다. 필 줄은 도구 단이 물러난 얼굴 위에는 서지
 * 않는다(`showGithubOnly`가 단을 감추는 그 얼굴들). */
function paintGithubFilters() {
  const chosen = activeGithubFilters();
  const count = el("task-gh-filter-count");
  count.textContent = String(chosen.length);
  count.hidden = chosen.length === 0;
  el("task-gh-filters").classList.toggle("is-active", chosen.length > 0);
  const pills = el("gh-filter-pills");
  pills.replaceChildren(...chosen.map(githubFilterPill));
  pills.hidden = el("github-tools").hidden || chosen.length === 0;
}

/* 한 필: 무엇을 좁혔는지 말하고, 누르면 그 하나만 걷는다. 단추 하나인 것은
 * 좁힌 것과 푸는 손이 한 자리여야 하기 때문이다. */
function githubFilterPill(filter) {
  const off = document.createElement("button");
  off.className = "task-chip task-gh-pill-off";
  off.type = "button";
  off.dataset.filter = filter.id;
  off.textContent = filter.name;
  off.setAttribute(
    "aria-label",
    t("task.github.filterRemove", "{{name}} 필터 제거", { name: filter.name }),
  );
  off.addEventListener("click", () => dropGithubFilter(filter.id));
  return off;
}

/* 값이 바뀌었으면 화면과 목록이 함께 따라온다. 하나만 하면 둘 중 하나가
 * 거짓말을 한다 — 필은 사라졌는데 목록은 좁혀진 채 남거나, 목록은 넓어졌는데
 * 배지가 여전히 둘을 센다. 기다림은 건너뛴다: 고르는 손은 이미 결정을 마쳤다. */
function githubFiltersChanged() {
  paintGithubFilters();
  loadTaskGithubNow();
}

/* 필 하나는 제 몫만 걷는다 — 나머지 좁힘은 그대로 서 있다. */
function dropGithubFilter(id) {
  if (id === "state") taskGithubFilters.state = null;
  if (id === "draft") taskGithubFilters.draft = false;
  if (id === "author") taskGithubFilters.author = null;
  if (id === "assignee") taskGithubFilters.assignee = null;
  githubFiltersChanged();
}

const ghFilterMenu = el("gh-filter-menu");

function closeGithubFilterMenu() {
  closing(ghFilterMenu);
  el("task-gh-filters").setAttribute("aria-expanded", "false");
}

/* 작성자·담당자의 후보 벤치(1-g70c). 폭의 첫 프로젝트 기준으로 한 번 읽고
 * 캐시한다 — 다중 폭에서 벤치는 저장소마다 다르지만, 이 팝오버는 하나만
 * 물을 수 있다(기록된 이탈). */
let ghUserBench = { home: null, logins: null, failed: false };

function loadGithubUserBench() {
  const home = ghProjectSpan(ghProjectHomes())[0]?.path ?? activeWorktreePath;
  if (!home || (ghUserBench.home === home && (ghUserBench.logins !== null || ghUserBench.loading))) {
    return;
  }
  ghUserBench = { home, logins: null, failed: false, loading: true };
  void (async () => {
    let logins = null;
    let failed = false;
    try {
      logins = await invoke("github_assignable_users", { project: home });
    } catch {
      failed = true;
    }
    if (ghUserBench.home !== home) return;
    ghUserBench = {
      home,
      logins: Array.isArray(logins) ? logins : [],
      failed,
      loading: false,
    };
    // 열려 있는 팝오버 위에 벤치가 도착했다 — 두 섹션의 자리만 갈아 끼운다.
    // 메뉴 전체를 다시 지으면 사람이 보고 있던 줄(호버·포커스)이 고아가 된다.
    if (!ghFilterMenu.hidden) paintGithubUserSections();
  })();
}

/* 두 사람-섹션이 서는 자리 하나 — 도착이 이 자리만 다시 그린다. */
function paintGithubUserSections() {
  const spot = el("gh-filter-users");
  if (!spot) return;
  spot.replaceChildren();
  githubUserSection(spot, t("task.github.filterAuthor", "작성자"), "author");
  githubUserSection(spot, t("task.github.filterAssignee", "담당자"), "assignee");
}

/* 한 사람 고르는 섹션 — 「모든」 줄이 null이고, 벤치가 못 서도 그 줄은
 * 남는다(필터의 문은 벤치 실패로 닫히지 않는다). */
function githubUserSection(host, said, id) {
  const rule = document.createElement("div");
  rule.className = "note-pop-rule";
  host.appendChild(rule);
  const head = document.createElement("div");
  head.className = "note-pop-label";
  head.textContent = said;
  host.appendChild(head);
  if (ghUserBench.logins === null) {
    menuRow(host, {
      said: t("task.github.filterLoading", "불러오는 중…"),
      disabled: true,
      close: closeGithubFilterMenu,
      run: () => {},
    });
    return;
  }
  if (ghUserBench.failed) {
    menuRow(host, {
      said: t("task.github.filterBenchFailed", "후보를 불러오지 못했습니다"),
      disabled: true,
      close: closeGithubFilterMenu,
      run: () => {},
    });
  }
  const anyWord = id === "author"
    ? t("task.github.filterAnyAuthor", "모든 작성자")
    : t("task.github.filterAnyAssignee", "모든 담당자");
  const rows = [
    { value: null, said: anyWord },
    ...ghUserBench.logins.map((login) => ({ value: login, said: login })),
  ];
  for (const one of rows) {
    const row = menuRow(host, {
      said: one.said,
      glyph: "check",
      close: closeGithubFilterMenu,
      run: () => {
        taskGithubFilters[id] = one.value;
        githubFiltersChanged();
      },
    });
    markGithubFilterRow(row, "menuitemradio", (taskGithubFilters[id] ?? null) === one.value);
  }
}

/* 팝오버는 열릴 때마다 다시 지어진다: 어느 줄이 켜져 있는지가 줄의 얼굴이고,
 * 지우기 줄은 지울 것이 있을 때에만 선다. */
function openGithubFilterMenu() {
  const host = el("gh-filter-menu-body");
  host.replaceChildren();
  // 줄이 열릴 때마다 새로 지어지므로 낱말도 그때의 언어로 적힌다 —
  // `say`가 지킬 것이 없다.
  const label = document.createElement("div");
  label.className = "note-pop-label";
  label.textContent = t("task.github.filterStatus", "상태");
  host.appendChild(label);
  // 아는 상태 셋에, 넷째로 「모든 상태」— 넷째 상태가 아니라 상태를 고르지 않은
  // 것이고, 그래서 값도 null이다. 낱말은 항목 다이얼로그가 배지에 쓰는 그
  // 낱말이다(`ghItemStateWord`): 같은 상태를 두 이름으로 부르면 고른 것과 화면에
  // 선 것이 서로 다른 말이 된다.
  const states = [
    ...GITHUB_FILTER_STATES.map((value) => ({ value, said: ghItemStateWord(value) })),
    { value: null, said: t("task.github.filterAnyState", "모든 상태") },
  ];
  for (const state of states) {
    const row = menuRow(host, {
      said: state.said,
      glyph: "check",
      close: closeGithubFilterMenu,
      run: () => pickGithubFilterState(state.value),
    });
    markGithubFilterRow(row, "menuitemradio", (taskGithubFilters.state ?? null) === state.value);
  }
  const rule = document.createElement("div");
  rule.className = "note-pop-rule";
  host.appendChild(rule);
  const draft = menuRow(host, {
    said: t("task.github.filterDraft", "초안만"),
    glyph: "check",
    close: closeGithubFilterMenu,
    run: () => {
      taskGithubFilters.draft = !taskGithubFilters.draft;
      githubFiltersChanged();
    },
  });
  markGithubFilterRow(draft, "menuitemcheckbox", taskGithubFilters.draft);
  // 작성자·담당자(1-g70c) — 벤치는 이 열림이 데운다.
  const users = document.createElement("div");
  users.id = "gh-filter-users";
  host.appendChild(users);
  paintGithubUserSections();
  loadGithubUserBench();
  // 지울 것이 없을 때의 지우기 줄은 가르치는 것이 없다.
  if (activeGithubFilters().length > 0) {
    const tail = document.createElement("div");
    tail.className = "note-pop-rule";
    host.appendChild(tail);
    menuRow(host, {
      said: t("task.github.filterClear", "필터 모두 지우기"),
      glyph: "x",
      close: closeGithubFilterMenu,
      run: clearGithubFilters,
    });
  }
  showing(ghFilterMenu);
  el("task-gh-filters").setAttribute("aria-expanded", "true");
  // 단추 아래에, 창 안으로 죄어서 — 이 창의 팝오버들이 함께 쓰는 그 손.
  placeUnder(ghFilterMenu, el("task-gh-filters"));
}

/* 켜진 줄만 체크를 보인다 — 자리는 모든 줄이 갖는다(CSS가 감춘다). 보조기술이
 * 읽는 것은 글리프가 아니라 `aria-checked`이므로 둘 다 적는다. */
function markGithubFilterRow(row, role, on) {
  row.setAttribute("role", role);
  row.setAttribute("aria-checked", String(on));
  row.dataset.on = on ? "yes" : "no";
}

function pickGithubFilterState(value) {
  taskGithubFilters.state = value;
  githubFiltersChanged();
}

function clearGithubFilters() {
  taskGithubFilters = { state: null, draft: false, author: null, assignee: null };
  githubFiltersChanged();
}

el("task-gh-filters").addEventListener("click", () => {
  if (overlayClosed(ghFilterMenu)) openGithubFilterMenu();
  else closeGithubFilterMenu();
});

dismissable(ghFilterMenu, closeGithubFilterMenu, el("task-gh-filters"));

// 이 창은 여전히 토큰을 들지 않는다 — 로그인은 터미널에서 gh의 손으로.
// 다만 그 터미널을 여는 것까지가 이 창의 몫이다("git hub 오르카는 자동으로
// 연결되던데"): 설정 카드의 그 문 하나를 여기서도 연다.
el("github-login").addEventListener("click", () => void openGithubLoginTerminal());
el("github-install").addEventListener("click", () => openExternal("https://cli.github.com"));

el("github-recheck").addEventListener("click", () => void recheckGithubPanel());

/* 「다시 시도」는 소스를 다시 찾아본다 — 원격이 방금 붙었거나, 확인하지
 * 못했던 저장소가 이번엔 답할 수 있다. 「다시 확인」과 한 손이다. */
el("github-retry").addEventListener("click", () => void recheckGithubPanel());

el("github-hide").addEventListener("click", () => setTaskSourceHidden("github", true));

/* ---- 작업판의 GitLab(1-g56b) ----------------------------------------------
 *
 * 문은 `glab`이다(1-g56a의 그 문 하나). 탭은 기계가 무엇을 가졌든 서고, 「없다」
 * 는 말은 판이 한다 — 바이너리 하나에 탭이 생겼다 사라지면 그 탭줄은 사람 몰래
 * 모양이 바뀐다.
 *
 * 보기 셋에 목록 셋(실측 1.4.180): MR이 먼저다. 체크아웃 위에 선 사람이 보는
 * 것이 MR이기 때문이고, 기본 칩이 「열기」인 것도 같은 이유다. 할 일은 이
 * 저장소의 것이 아니다 — GitLab의 할 일은 볼 수 있는 모든 프로젝트를 가로지르는
 * 받은 편지함이라, 그 행만 프로젝트 이름을 함께 그린다.
 *
 * 질문을 짓는 손은 여기가 아니다: 보기와 칩은 **고른 값**으로 건너가고, 그것을
 * 질의로 옮기는 것은 `glab.rs` 하나다(GitHub 쪽이 `gh::search_plan`에 대해 적어
 * 둔 그 이유 그대로). */
const GITLAB_SURFACES = [
  "gitlab-loading",
  "gitlab-auth",
  "gitlab-quiet",
  "gitlab-error",
  "gitlab-none",
  "gitlab-list",
];

/* 도구 단이 설 수 있는 얼굴들. 「기다리는 중」이 드는 것은 필수다 — 새로 고침
 * 한 번에 단이 사라지면 방금 누른 단추가 화면에서 없어진다. */
const GITLAB_TOOL_SURFACES = new Set(["gitlab-loading", "gitlab-none", "gitlab-list"]);

function showGitlabOnly(id, said) {
  showOneSurface(GITLAB_SURFACES, id, said);
  el("gitlab-tools").hidden = !GITLAB_TOOL_SURFACES.has(id);
}

/* 보기가 아는 칩. 이슈의 칩은 담당자를 옮기고(상태는 언제나 열림), MR의 칩은
 * 상태를 옮긴다 — 실측이고, 이 표가 그 사실의 유일한 사본이다. 할 일에는 칩이
 * 없다: 대기 중인 것 말고 물을 것이 없다. */
const GITLAB_FILTERS = {
  issues: [
    ["opened", () => t("task.gitlab.filterOpen", "열기")],
    ["assigned-to-me", () => t("task.gitlab.filterAssigned", "나에게 할당됨")],
  ],
  mrs: [
    ["opened", () => t("task.gitlab.filterOpen", "열기")],
    ["merged", () => t("task.gitlab.filterMerged", "병합됨")],
    ["closed", () => t("task.gitlab.filterClosed", "닫힘")],
    ["all", () => t("task.gitlab.filterAll", "모두")],
  ],
  todos: [],
};

/* 기본은 MR과 열기(실측). 보기를 옮기면 칩은 그 보기의 첫 칩으로 돌아간다 —
 * 이슈에 「병합됨」이 남아 있으면 그 칩이 부를 목록이 없다. */
let taskGitlabView = "mrs";
let taskGitlabFilter = "opened";

/* 물은 것과 온 답의 궤가 다르면 답은 버린다(Jira의 jiraIssuesGeneration 관용).
 * 칩을 두 번 누르는 동안 먼저 나간 질문이 나중에 돌아오는 일이 이 판에서는
 * 흔하다 — 셋을 오가는 토글이 하나의 목록을 공유하기 때문이다. */
let taskGitlabGeneration = 0;

/* glab의 답은 실행당 한 번. 설정 카드가 이미 물었으면 그 답을 쓴다. */
let gitlabStatusLoaded = false;

function ensureGitlabStatus() {
  if (gitlabStatusLoaded) return Promise.resolve();
  return refreshGitlabIntegration();
}

/* 아이콘 하나뿐인 손에는 말이 함께 선다 — 화면 밖의 한 줄로.
 *
 * `aria-label`이 아니라 `.sr` 글자인 것이 요점이다: 속성은 언어가 바뀔 때
 * 아무도 다시 쓰지 않고, `say`가 든 글자는 다시 쓰인다. 이 목록은 언어를
 * 바꾼다고 다시 그려지지 않으므로, 라벨을 속성에 두면 그 순간부터 틀린 말이
 * 화면 뒤에 남는다. */
function gitlabActButton(glyph, produce, run) {
  const button = document.createElement("button");
  button.type = "button";
  button.className = "task-gl-act";
  button.innerHTML = icon(glyph);
  const label = document.createElement("span");
  label.className = "sr";
  say(label, produce);
  button.appendChild(label);
  button.addEventListener("click", (event) => {
    // 행의 손짓이 따로 있는 판이다(할 일의 행은 열린다) — 그 위로 겹치지 않게.
    event.stopPropagation();
    run();
  });
  return button;
}

/* `!42`는 MR, `#42`는 이슈 — GitLab이 제 화면에서 쓰는 그 기호다. 번호는
 * iid(프로젝트 안의 번호)이고, 전역 id는 사람이 보는 자리에 없다. */
function gitlabItemId(item) {
  return `${item.kind === "mr" ? "!" : "#"}${item.number}`;
}

function gitlabKindWord(kind) {
  return kind === "mr"
    ? t("task.gitlab.kindMr", "MR")
    : t("task.gitlab.kindIssue", "이슈");
}

/* 날짜 한 칸. 읽는 사람의 달력으로 — 일정 화면의 그 줄과 같은 손이다. 읽을 수
 * 없는 값은 빈칸으로 남는다: "Invalid Date"는 사람에게 아무 말도 하지 않는다. */
function gitlabDay(iso) {
  const at = Date.parse(iso ?? "");
  return Number.isNaN(at) ? "" : new Date(at).toLocaleDateString();
}

function gitlabItemRow(item) {
  // A `div` with two real buttons in it: a `<button>` row could not hold them,
  // and the row's own gesture — the item dialog — stands on `actsAsButton` so
  // the keyboard reaches it too.
  const row = document.createElement("div");
  row.className = "task-gl-row";
  row.dataset.kind = item.kind;
  row.dataset.number = String(item.number);

  const id = document.createElement("span");
  id.className = "task-gl-id";
  id.textContent = gitlabItemId(item);

  const title = document.createElement("span");
  title.className = "task-gl-title";
  title.textContent = item.title ?? "";

  // 종류와 상태가 한 칸에 선다. 상태는 인스턴스가 한 말 그대로다 — opened·
  // merged·locked를 여기서 옮겨 적으면, 우리가 모르는 상태가 오는 날 화면이
  // 조용히 틀린다.
  const kind = document.createElement("span");
  kind.className = "task-gl-kind";
  say(kind, () => [gitlabKindWord(item.kind), item.state].filter(Boolean).join(" · "));

  const when = document.createElement("span");
  when.className = "task-gl-when";
  when.textContent = gitlabDay(item.updated_at);

  const acts = document.createElement("span");
  acts.className = "task-gl-acts";
  acts.append(
    // 시작은 Jira 행이 쓰는 그 문이다(`startWorktreeFromWorkItem`): URL이
    // 컴포저의 스마트 칸으로 들어가고, 이름을 짓는 손은 Rust 하나뿐이다.
    gitlabActButton(
      "forward",
      () => t("task.gitlab.startWork", "{{item}}에서 워크스페이스 시작", {
        item: gitlabItemId(item),
      }),
      () => void startWorktreeFromProviderItem(item, "gitlab"),
    ),
    gitlabActButton(
      "external",
      () => t("task.gitlab.openWeb", "GitLab에서 열기"),
      () => openExternal(item.url),
    ),
  );

  row.append(id, title, kind, when, acts);
  // 행을 누르는 것은 그 항목을 여는 것이다(실측). 손 두 개는 제 일이 따로
  // 있으므로, 그 위에서 시작한 클릭은 이 문으로 오지 않는다.
  actsAsButton(row, (event) => {
    if (event.target.closest(".task-gl-act")) return;
    openGitlabItem(item);
  });
  return row;
}

function gitlabTodoRow(todo) {
  const row = document.createElement("div");
  row.className = "task-gl-row task-gl-todo";

  // `marked_for_review` 는 사람이 읽는 말이 아니다 — Orca도 밑줄을 공백으로
  // 바꿔 그대로 세운다(실측). 번역하지 않는 것은 이것이 GitLab의 어휘이고,
  // 이 창이 아는 목록이 아니기 때문이다.
  const action = document.createElement("span");
  action.className = "task-gl-action";
  action.textContent = (todo.action ?? "").replaceAll("_", " ");

  const title = document.createElement("span");
  title.className = "task-gl-title";
  title.textContent = todo.target_title ?? "";

  // 할 일은 저장소를 가로지르므로, 어느 프로젝트의 것인지가 행에 있어야 한다.
  const project = document.createElement("span");
  project.className = "task-gl-project";
  project.textContent = todo.project_path ?? "";

  const when = document.createElement("span");
  when.className = "task-gl-when";
  when.textContent = gitlabDay(todo.updated_at);

  const acts = document.createElement("span");
  acts.className = "task-gl-acts";
  acts.appendChild(gitlabActButton(
    "external",
    () => t("task.gitlab.openWeb", "GitLab에서 열기"),
    () => openExternal(todo.target_url),
  ));

  row.append(action, title, project, when, acts);
  // 할 일의 행은 그 페이지다(실측: 다이얼로그가 없다). `actsAsButton`으로
  // 서야 키보드에도 존재한다 — 클릭 핸들러만 단 div는 아무에게도 닿지 않는다.
  actsAsButton(row, (event) => {
    if (event.target.closest(".task-gl-act")) return;
    openExternal(todo.target_url);
  });
  return row;
}

/* 도구 단은 한 자리에서만 그려진다: 무엇을 물으러 가든 이 손이 그 질문을
 * 말한다. 칩도 새로 고침의 라벨도 `say`가 들고 있으므로, 언어가 바뀌어도
 * 이 판은 스스로 제 말을 다시 한다. */
function paintGitlabTools() {
  selectSegment("gitlab-views", taskGitlabView);
  const chips = el("gitlab-chips");
  const known = GITLAB_FILTERS[taskGitlabView] ?? [];
  chips.hidden = known.length === 0;
  chips.replaceChildren(...known.map(([value, produce]) => {
    const chip = document.createElement("button");
    chip.type = "button";
    chip.className = "task-chip";
    chip.dataset.gitlabFilter = value;
    const on = value === taskGitlabFilter;
    chip.classList.toggle("is-active", on);
    chip.setAttribute("aria-pressed", String(on));
    say(chip, produce);
    return chip;
  }));
  const todos = taskGitlabView === "todos";
  say(el("gitlab-refresh-label"), () => (todos
    ? t("task.gitlab.refreshTodos", "내 할 일 새로 고침")
    : t("task.gitlab.refresh", "GitLab 작업 항목 새로 고침")));
}

/* 빈 답이 무엇의 빈 답인지는 어느 목록을 물었느냐에 달렸다. 할 일이 비었다는
 * 것은 좋은 소식이고(실측 문구), 이슈와 MR이 비었다는 것은 이 필터의 사정이다. */
function sayGitlabNone() {
  const title = el("gitlab-none-title");
  const body = el("gitlab-none-body");
  if (taskGitlabView === "todos") {
    say(title, () => t("task.gitlab.emptyTodos", "대기 중인 할 일 없음"));
    say(body, () => t(
      "task.gitlab.emptyTodosCaughtUp",
      "대기 중인 할 일이 없습니다. 모두 따라잡았네요!",
    ));
    return;
  }
  const issues = taskGitlabView === "issues";
  say(title, () => (issues
    ? t("task.gitlab.noIssues", "GitLab 이슈 없음")
    : t("task.gitlab.noMrs", "GitLab MR 없음")));
  say(body, () => (issues
    ? t("task.gitlab.emptyIssues", "이 필터와 일치하는 GitLab 이슈가 없습니다.")
    : t("task.gitlab.emptyMrs", "이 필터와 일치하는 GitLab MR이 없습니다.")));
}

/* 백엔드가 보낸 것은 **갈래**뿐이다(`glab`이 한 말은 경계를 넘지 않는다), 그래서
 * 문장은 이 표가 짓는다. 모르는 갈래는 빈 글자로 — 부르는 쪽이 제 자리의 일반
 * 문장을 세운다(이쪽 실수라면 그쪽이 실어 보낸 우리 말이 있다).
 *
 * 표가 하나인 것이 요점이다: 목록과 항목 카드가 같은 거절에 다른 말을 하면,
 * 사람은 같은 사정을 두 번 다르게 배운다. */
function gitlabTroubleWord(failure) {
  return {
    forbidden: () => t(
      "task.gitlab.errForbidden",
      "이 프로젝트의 항목을 읽을 권한이 없습니다. GitLab 토큰 범위를 확인하세요.",
    ),
    no_project: () => t(
      "task.gitlab.errNoProject",
      "이 저장소에서 GitLab 프로젝트를 찾지 못했습니다.",
    ),
    rate_limited: () => t(
      "task.gitlab.errRateLimited",
      "GitLab 속도 제한에 걸렸습니다. 잠시 후 다시 시도하세요.",
    ),
    network: () => t("task.gitlab.errNetwork", "네트워크 오류 — 연결을 확인하세요."),
  }[failure.kind]?.() ?? "";
}

function sayGitlabTrouble(failure) {
  // 설치되지 않은 CLI는 실패가 아니라 사정이다 — 판이 열릴 때와 같은 조용한 줄.
  if (failure.kind === "missing") {
    showGitlabOnly("gitlab-quiet", t(
      "task.gitlab.installFirst",
      "GitLab CLI(glab)를 설치하면 이슈와 MR이 여기 섭니다.",
    ));
    return;
  }
  showGitlabOnly(
    "gitlab-error",
    gitlabTroubleWord(failure) || failure.message
      || t("task.gitlab.errFailed", "GitLab 항목을 읽지 못했습니다."),
  );
}

async function loadTaskGitlab() {
  paintGitlabTools();
  const project = activeWorktreePath;
  if (!project) {
    showGitlabOnly(
      "gitlab-quiet",
      t("task.gitlab.noProject", "프로젝트를 열면 그 저장소의 이슈와 MR이 여기 섭니다."),
    );
    return;
  }
  const generation = ++taskGitlabGeneration;
  showGitlabOnly("gitlab-loading");
  const todos = taskGitlabView === "todos";
  let rows;
  try {
    rows = todos
      ? await invoke("gitlab_todos", { project })
      : await invoke("gitlab_work_items", {
        project,
        view: taskGitlabView,
        filter: taskGitlabFilter,
      });
  } catch (error) {
    if (generation !== taskGitlabGeneration) return;
    sayGitlabTrouble(failureOf(error));
    return;
  }
  if (generation !== taskGitlabGeneration) return;
  const items = Array.isArray(rows) ? rows : [];
  if (items.length === 0) {
    sayGitlabNone();
    showGitlabOnly("gitlab-none");
    return;
  }
  showGitlabOnly("gitlab-list");
  el("gitlab-list").replaceChildren(...items.map(todos ? gitlabTodoRow : gitlabItemRow));
}

/* 판이 서는 순간의 첫 질문은 「이 기계에 glab이 있는가」다. 없으면 목록을 묻지
 * 않는다 — 없는 CLI에 던진 질문의 답은 사람이 할 일을 이름하지 못한다. */
async function paintGitlabPanel() {
  showGitlabOnly("gitlab-loading");
  await ensureGitlabStatus();
  if (gitlabIntegrationError !== null) {
    showGitlabOnly("gitlab-error", gitlabIntegrationError);
    return;
  }
  if (!gitlabStatus.installed) {
    showGitlabOnly("gitlab-quiet", t(
      "task.gitlab.installFirst",
      "GitLab CLI(glab)를 설치하면 이슈와 MR이 여기 섭니다.",
    ));
    return;
  }
  if (!gitlabStatus.authenticated) {
    showGitlabOnly("gitlab-auth");
    return;
  }
  await loadTaskGitlab();
}

/* 보기를 옮기는 것은 칩을 처음으로 되돌리는 것이다 — 그 보기의 첫 칩이 곧
 * 실측의 기본값이고, 남아 있던 칩은 새 보기에서 부를 목록이 없다. */
el("gitlab-views").addEventListener("click", (event) => {
  const button = event.target.closest(".segment-btn");
  if (!button) return;
  taskGitlabView = button.dataset.value;
  taskGitlabFilter = GITLAB_FILTERS[taskGitlabView]?.[0]?.[0] ?? "opened";
  void loadTaskGitlab();
});

el("gitlab-chips").addEventListener("click", (event) => {
  const chip = event.target.closest("[data-gitlab-filter]");
  if (!chip) return;
  taskGitlabFilter = chip.dataset.gitlabFilter;
  void loadTaskGitlab();
});

el("gitlab-refresh").addEventListener("click", () => void loadTaskGitlab());
el("gitlab-login").addEventListener("click", () => {
  setTaskOpen(false);
  void openGitlabLoginTerminal();
});
el("gitlab-hide").addEventListener("click", () => setTaskSourceHidden("gitlab", true));

/* ---- GitLab 항목 다이얼로그(1-g56c) ---------------------------------------
 *
 * GitHub 쪽 다이얼로그의 쌍둥이다: 행 하나가 상세를 열고, 본문과 대화가 이
 * 창의 마크다운으로 그려지며, 댓글과 닫기/다시 열기가 `glab`의 손으로 나간다.
 * 카드도 스크림도 그쪽과 같은 것을 입는다 — 두 벌의 다이얼로그 모양은 반드시
 * 어긋나고, 어긋난 뒤에는 어느 쪽이 옳은지 아무도 말할 수 없다.
 *
 * 다른 것은 셋이다. 라벨 칩이 한 줄 서고, 대화의 목소리는 스레드의 해결
 * 여부를 함께 들며, 닫기/다시 열기가 **두 종류 모두**에 선다(실측 화면은 MR의
 * 푸터에만 두지만, `glab issue close`는 이슈에도 있는 문이다).
 *
 * 파일·파이프라인 탭은 이 조각의 것이 아니다. */
const glItemScrim = el("gl-item-scrim");
let glItem = null;
let glItemAsking = null;
let glItemBusy = false;
/* 어느 판이 서 있나 — 카드가 열릴 때마다 설명으로 돌아온다. */
let glItemTab = "body";
/* 파이프라인은 처음 그 판을 열 때 한 번 묻고, 로그는 잡마다 캐시된다 —
 * 펼칠 때마다 같은 로그를 다시 실어 오는 왕복은 사람의 기다림이다. */
let glItemJobs = null;
let glJobTraces = new Map();
let glJobOpen = null;
/* 설명 판은 읽기와 고쳐쓰기 중 하나로 선다 — MR만 고쳐 쓴다(실측). */
let glItemEditing = false;
/* 승인·바뀐 파일의 늦은 한 번 읽기(파이프라인의 그 판) — null은 아직이다. */
let glItemReview = null;
/* 관리를 누른 손에게만 벤치를 읽는다 — null이면 벤치는 접혀 있다. */
let glItemMembers = null;

/* `!42`·`#7` — 목록 행이 쓰는 그 기호를 그대로 쓴다(`gitlabItemId`). */
function glItemRef(item) {
  return gitlabItemId({ kind: item.kind, number: item.number });
}

function openGitlabItem(item) {
  glItem = {
    ...item,
    project: activeWorktreePath,
  };
  showModal(glItemScrim, { animated: true });
  // 목록이 이미 아는 사실로 먼저 서고, 상세가 도착하면 그 위를 덮는다 — 빈
  // 카드가 한 박자 서 있는 것보다, 아는 만큼 선 카드가 채워지는 쪽이다.
  paintGitlabItemHead(item);
  el("gl-item-meta").textContent = "";
  el("gl-item-labels").hidden = true;
  el("gl-item-labels").replaceChildren();
  el("gl-item-body").replaceChildren();
  el("gl-item-comments-head").textContent = "";
  el("gl-item-comments").replaceChildren();
  el("gl-item-error").hidden = true;
  el("gl-item-state-act").hidden = true;
  glItemTab = "body";
  glItemJobs = null;
  glJobTraces = new Map();
  glJobOpen = null;
  el("gl-item-tab-pipeline").hidden = true;
  el("gl-item-jobs").replaceChildren();
  glItemEditing = false;
  el("gl-item-edit").hidden = true;
  el("gl-item-edit-form").hidden = true;
  el("gl-item-merge").hidden = true;
  glItemReview = null;
  glItemMembers = null;
  el("gl-item-tab-files").hidden = true;
  el("gl-item-files").replaceChildren();
  el("gl-item-reviewers").hidden = true;
  el("gl-inline").hidden = true;
  el("gl-inline-line").value = "";
  el("gl-inline-body").value = "";
  el("gl-inline-line").placeholder = t("task.gitlab.inlineLinePh", "줄");
  el("gl-inline-body").placeholder = t("task.gitlab.inlineBodyPh", "인라인 댓글");
  paintGlFilesTabWord();
  paintGlItemTabs();
  el("gl-item-comment-field").placeholder = t(
    "task.gitlab.commentPlaceholder",
    "{{ref}}에 댓글…",
    { ref: glItemRef(glItem) },
  );
  void readGitlabItem();
}

function closeGitlabItem() {
  // 나가는 중인 요청이 있으면 닫지 않는다: 댓글이 나가는 동안 사라진 카드는
  // 사람에게 그것이 갔는지 안 갔는지 말해 줄 자리가 없다(gh 쪽의 그 규칙).
  if (glItemBusy) return;
  hideModal(glItemScrim, { animated: true });
  glItem = null;
  glItemAsking = null;
  el("gl-item-comment-field").value = "";
}

/* 머리는 두 번 그려진다 — 행이 아는 만큼 먼저, 상세가 온 뒤 다시. 상태 낱말은
 * 인스턴스가 한 말 그대로다(목록 행의 그 규칙): `opened`·`merged`·`locked`를
 * 여기서 옮겨 적으면 우리가 모르는 상태가 오는 날 화면이 조용히 틀린다. */
function paintGitlabItemHead(item) {
  el("gl-item-number").textContent = glItemRef(item);
  el("gl-item-title").textContent = item.title ?? "";
  const pill = el("gl-item-state");
  pill.className = `wt-gh-pill gl-item-state is-${item.state ?? ""}`;
  pill.textContent = item.state ?? "";
}

async function readGitlabItem() {
  const at = glItem;
  if (!at) return;
  // 물은 것과 온 답의 궤가 다르면 답은 버린다 — 목록이 쓰는 그 세대 규칙과
  // 같은 이유다: 카드를 닫고 다른 행을 열면 먼저 나간 질문이 나중에 온다.
  const asking = [at.project, at.kind, at.number].join(" ");
  glItemAsking = asking;
  el("gl-item-loading").hidden = false;
  el("gl-item-error").hidden = true;
  let detail;
  try {
    detail = await invoke("gitlab_item_detail", {
      project: at.project,
      kind: at.kind,
      number: at.number,
    });
  } catch (error) {
    if (glItemAsking !== asking) return;
    el("gl-item-loading").hidden = true;
    sayGitlabItemTrouble(error, () =>
      t("task.gitlab.detailFailed", "항목을 불러오지 못했습니다"));
    return;
  }
  if (glItemAsking !== asking) return;
  el("gl-item-loading").hidden = true;
  paintGitlabItem(detail);
}

/* 실패는 갈래로 온다(`glab`이 한 말은 경계를 넘지 않는다). 목록이 세우는 그
 * 문장들을 이 카드도 쓰고, 그 표에 없는 갈래만 이 자리의 문장으로 답한다. */
function sayGitlabItemTrouble(error, fallback) {
  const failure = failureOf(error);
  const line = el("gl-item-error");
  line.hidden = false;
  say(line, () => gitlabTroubleWord(failure) || failure.message || fallback());
}

function glCommentNode(comment) {
  // 「해결됨」만 그린다. 표가 없는 것은 「해결되지 않음」이 아니라 「스레드가
  // 아니다」이므로, 반대말을 세우면 평범한 댓글이 미결로 보인다.
  let badge = null;
  if (comment.resolved === true) {
    badge = document.createElement("span");
    badge.className = "gl-item-resolved";
    say(badge, () => t("task.gitlab.resolved", "해결됨"));
  }
  return itemVoiceCard("gl", () => [
    comment.author,
    gitlabDay(comment.created_at),
  ].filter(Boolean).join(" · "), comment.body ?? "", badge);
}

function paintGitlabItem(detail) {
  if (!glItem) return;
  glItem.state = detail.state;
  glItem.url = detail.url || glItem.url;
  // 편집 폼은 상세가 말한 원문에서 시작한다 — 화면의 마크다운을 되읽는
  // 길은 왕복마다 원문을 잃는다.
  glItem.told = detail;
  paintGitlabItemHead(detail);
  say(el("gl-item-meta"), () => [detail.author, gitlabDay(detail.created_at)]
    .filter(Boolean).join(" · "));

  const labels = el("gl-item-labels");
  labels.replaceChildren(...detail.labels.map((name) => {
    const chip = document.createElement("span");
    chip.className = "gl-item-label";
    chip.textContent = name;
    return chip;
  }));

  const body = el("gl-item-body");
  body.replaceChildren();
  if (detail.body.trim()) {
    paintMarkdown(body, detail.body);
  } else {
    const quiet = document.createElement("p");
    quiet.className = "gl-item-quiet";
    say(quiet, () => t("task.gitlab.noBody", "설명이 없습니다."));
    body.appendChild(quiet);
  }

  say(el("gl-item-comments-head"), () =>
    t("task.gitlab.comments", "댓글 {{count}}", { count: detail.comments.length }));
  el("gl-item-comments").replaceChildren(...detail.comments.map(glCommentNode));
  // 탭의 낱말에도 그 수가 실린다(실측: "대화(+수)").
  say(el("gl-item-tab-talk"), () =>
    t("task.gitlab.tabTalk", "대화 {{count}}", { count: detail.comments.length }));
  // 파이프라인 탭은 MR이 머리 파이프라인을 이름했을 때만 선다 — 이슈에는
  // 그 사실 자체가 없다.
  glItem.pipeline = detail.pipeline_id ?? null;
  el("gl-item-tab-pipeline").hidden = glItem.pipeline == null;
  paintGlItemTabs();

  // 닫힌 것은 다시 열 수 있고, 열린 것은 닫을 수 있다 — 병합된 MR만 둘 다
  // 아니다. 권한의 최종 판정은 `glab`의 몫이고, 여기는 문만 세운다.
  const act = el("gl-item-state-act");
  const closed = detail.state === "closed";
  act.hidden = detail.state === "merged";
  act.dataset.wantOpen = String(closed);
  say(act, () => (closed
    ? t("task.gitlab.reopen", "다시 열기")
    : t("task.gitlab.close", "닫기")));

  // 다시 읽힌 카드는 읽기 판으로 돌아온다 — 저장이 성공한 그 길이다.
  // 병합은 열려 있는 MR의 문이고, 권한의 최종 판정은 `glab`의 몫이다.
  glItemEditing = false;
  paintGlItemEdit();
  el("gl-item-merge").hidden = !(glItem.kind === "mr" && detail.state === "opened");

  // 파일 탭은 MR의 것이고, 수는 늦은 읽기가 도착하며 실린다.
  el("gl-item-tab-files").hidden = glItem.kind !== "mr";
  paintGlReviewers();
  paintGlItemFiles();
  if (glItem.kind === "mr") void loadGlMrReview();
}

/* 리뷰어 카드(실측) — 칩·승인 줄·규칙. 벤치는 관리를 누른 손에게만. */
function paintGlReviewers() {
  const card = el("gl-item-reviewers");
  if (glItem?.kind !== "mr") {
    card.hidden = true;
    return;
  }
  card.hidden = false;
  const held = glItem.told?.reviewers ?? [];
  const chips = el("gl-item-reviewer-chips");
  if (held.length === 0) {
    const quiet = document.createElement("span");
    quiet.className = "gl-item-quiet";
    say(quiet, () => t("task.gitlab.noReviewers", "리뷰어가 없습니다."));
    chips.replaceChildren(quiet);
  } else {
    chips.replaceChildren(...held.map((reviewer) => {
      const chip = document.createElement("span");
      chip.className = "gl-item-reviewer";
      const name = document.createElement("span");
      name.textContent = reviewer.username;
      const drop = document.createElement("button");
      drop.type = "button";
      drop.className = "gl-item-reviewer-drop";
      drop.textContent = "×";
      drop.setAttribute("aria-label",
        t("task.gitlab.removeReviewer", "리뷰어 {{name}} 제거", { name: reviewer.username }));
      drop.addEventListener("click", () => void setGlReviewers(
        held.filter((one) => one.id !== reviewer.id).map((one) => one.id)));
      chip.append(name, drop);
      return chip;
    }));
  }

  // 승인 줄: 두 끝점이 답한 만큼만 선다(실측의 그 문장 조립).
  const line = el("gl-item-approvals");
  const approvals = glItemReview?.approvals ?? null;
  line.hidden = approvals === null;
  if (approvals) {
    say(line, () => {
      if (approvals.left === 0) return t("task.gitlab.approved", "승인 완료");
      const remaining = t("task.gitlab.approvalsLeft", "승인 {{left}} 남음",
        { left: approvals.left ?? 0 });
      return approvals.required == null
        ? remaining
        : remaining + t("task.gitlab.approvalsOf", " · {{required}} 필요",
          { required: approvals.required });
    });
  }

  // 규칙들: 이름과 그 서 있는 자리.
  const rules = el("gl-item-approval-rules");
  const heldRules = approvals?.rules ?? [];
  rules.hidden = heldRules.length === 0;
  rules.replaceChildren(...heldRules.map((rule) => {
    const row = document.createElement("div");
    row.className = "gl-item-approval-rule";
    const name = document.createElement("span");
    name.className = "gl-item-approval-rule-name";
    name.textContent = rule.name;
    const stand = document.createElement("span");
    say(stand, () => (rule.approved
      ? t("task.gitlab.approved", "승인 완료")
      : t("task.gitlab.ruleNeeds", "{{count}} 필요", { count: rule.required })));
    row.append(name, stand);
    return row;
  }));

  paintGlReviewerBench();
}

/* 벤치: 이미 앉은 사람은 후보에서 빠진다(실측의 그 차집합). */
function paintGlReviewerBench() {
  const bench = el("gl-item-reviewer-bench");
  bench.hidden = glItemMembers === null;
  if (glItemMembers === null) return;
  const seated = new Set((glItem?.told?.reviewers ?? []).map((one) => one.id));
  const pick = el("gl-item-reviewer-pick");
  pick.replaceChildren(...[
    (() => {
      const door = document.createElement("option");
      door.value = "";
      say(door, () => t("task.gitlab.addReviewer", "리뷰어 추가"));
      return door;
    })(),
    ...glItemMembers
      .filter((one) => !seated.has(one.id))
      .map((one) => {
        const row = document.createElement("option");
        row.value = String(one.id);
        row.textContent = one.username;
        return row;
      }),
  ]);
}

/* 읽기와 고쳐쓰기 중 하나만 선다. 라벨·본문이 물러난 자리에 폼이 서고,
 * [편집]은 MR에만 — 이슈는 이 창에서 읽기 전용이다(실측). */
function paintGlItemEdit() {
  const editing = glItemEditing;
  el("gl-item-edit-form").hidden = !editing;
  el("gl-item-edit").hidden = editing || glItem?.kind !== "mr";
  el("gl-item-labels").hidden = editing || (glItem?.told?.labels ?? []).length === 0;
  el("gl-item-body").hidden = editing;
}

/* 세 판 중 하나가 선다. 컴포저와 푸터는 탭 밖이다 — 실측의 그 해부. */
function paintGlItemTabs() {
  for (const button of el("gl-item-tabs").querySelectorAll("[data-gl-tab]")) {
    const here = button.dataset.glTab === glItemTab;
    button.classList.toggle("is-active", here);
    button.setAttribute("aria-selected", here ? "true" : "false");
  }
  el("gl-item-pane-body").hidden = glItemTab !== "body";
  el("gl-item-pane-talk").hidden = glItemTab !== "talk";
  el("gl-item-pane-files").hidden = glItemTab !== "files";
  el("gl-item-pane-pipeline").hidden = glItemTab !== "pipeline";
}

el("gl-item-tabs").addEventListener("click", (event) => {
  const button = event.target.closest("[data-gl-tab]");
  if (!button || button.dataset.glTab === glItemTab) return;
  glItemTab = button.dataset.glTab;
  paintGlItemTabs();
  if (glItemTab === "pipeline" && glItemJobs === null) void loadGlItemJobs();
});

/* 잡의 상태는 인스턴스의 낱말 그대로 서고, 아는 네 낱말만 제 색을 입는다 —
 * 카드의 상태 낱말이 지키는 그 규칙이다. */
const GL_JOB_TONES = new Set(["success", "failed", "running", "manual"]);

function glJobDuration(seconds) {
  if (seconds == null) return "";
  const whole = Math.round(seconds);
  if (whole < 60) return `${whole}s`;
  return `${Math.floor(whole / 60)}m ${whole % 60}s`;
}

function glJobNode(job) {
  const row = document.createElement("div");
  row.className = "gl-item-job";
  const head = document.createElement("button");
  head.type = "button";
  head.className = "gl-item-job-head";
  head.setAttribute("aria-expanded", glJobOpen === job.id ? "true" : "false");
  const name = document.createElement("span");
  name.className = "gl-item-job-name";
  name.textContent = job.name;
  const stage = document.createElement("span");
  stage.className = "gl-item-job-stage";
  stage.textContent = job.stage;
  const when = document.createElement("span");
  when.className = "gl-item-job-when";
  when.textContent = glJobDuration(job.duration);
  const state = document.createElement("span");
  state.className = `wt-gh-pill gl-item-job-state${GL_JOB_TONES.has(job.status) ? ` is-${job.status}` : ""}`;
  state.textContent = job.status;
  head.append(name, stage, when, state);
  head.addEventListener("click", () => void toggleGlJobLog(job));
  const acts = document.createElement("span");
  acts.className = "gl-item-job-acts";
  const retry = document.createElement("button");
  retry.type = "button";
  retry.className = "btn btn--ghost";
  say(retry, () => t("task.gitlab.retryJob", "다시 시도"));
  retry.addEventListener("click", () => void retryGlJob(job, retry));
  const web = document.createElement("button");
  web.type = "button";
  web.className = "btn btn--ghost";
  web.textContent = "↗";
  web.dataset.tip = t("task.gitlab.openWeb", "GitLab에서 열기");
  web.setAttribute("aria-label", t("task.gitlab.openWeb", "GitLab에서 열기"));
  web.addEventListener("click", () => openExternal(job.web_url));
  acts.append(retry, web);
  row.append(head, acts);
  if (glJobOpen === job.id) {
    const log = document.createElement("pre");
    log.className = "gl-item-job-log";
    log.textContent = glJobTraces.get(job.id)
      ?? t("usage.loading", "확인 중…");
    row.appendChild(log);
  }
  return row;
}

/* 한 파일의 카드: 경로(개명이면 이전 경로), +N −N, 그리고 diff 전문 —
 * 접힘 없이 선다(실측). 잡히지 않은 diff는 짐작 대신 문장으로. */
function glFileNode(file) {
  const card = document.createElement("div");
  card.className = "gl-item-file";
  const head = document.createElement("div");
  head.className = "gl-item-file-head";
  const names = document.createElement("div");
  names.className = "gl-item-file-names";
  const path = document.createElement("div");
  path.className = "gl-item-file-path";
  path.textContent = file.path;
  names.appendChild(path);
  if (file.old_path) {
    const from = document.createElement("div");
    from.className = "gl-item-file-from";
    say(from, () => t("task.gitlab.fromPath", "이전: {{path}}", { path: file.old_path }));
    names.appendChild(from);
  }
  const counts = document.createElement("div");
  counts.className = "gl-item-file-counts";
  const grown = document.createElement("span");
  grown.className = "gl-item-file-plus";
  grown.textContent = `+${file.additions}`;
  const gone = document.createElement("span");
  gone.className = "gl-item-file-minus";
  gone.textContent = `-${file.deletions}`;
  counts.append(grown, gone);
  head.append(names, counts);
  card.appendChild(head);
  if (file.diff) {
    const body = document.createElement("pre");
    body.className = "gl-item-job-log gl-item-file-diff";
    body.textContent = file.diff;
    card.appendChild(body);
  } else {
    const quiet = document.createElement("p");
    quiet.className = "gl-item-quiet";
    say(quiet, () => t("task.gitlab.noDiff", "diff 내용이 없습니다."));
    card.appendChild(quiet);
  }
  return card;
}

function paintGlItemFiles() {
  const host = el("gl-item-files");
  const files = glItemReview?.files ?? null;
  // 폼은 바뀐 파일이 있어야 선다(실측) — 콤보의 첫 옵션은 폼의 이름이다.
  const form = el("gl-inline");
  form.hidden = !files || files.length === 0;
  if (!form.hidden) {
    const pick = el("gl-inline-file");
    const chosen = pick.value;
    pick.replaceChildren(...[
      (() => {
        const door = document.createElement("option");
        door.value = "";
        say(door, () => t("task.gitlab.inlineFile", "파일"));
        return door;
      })(),
      ...files.map((file) => {
        const row = document.createElement("option");
        row.value = file.path;
        row.textContent = file.path;
        return row;
      }),
    ]);
    pick.value = files.some((file) => file.path === chosen) ? chosen : "";
  }
  if (files === null) {
    host.replaceChildren();
    return;
  }
  if (files.length === 0) {
    const quiet = document.createElement("p");
    quiet.className = "gl-item-quiet";
    say(quiet, () => t("task.gitlab.noFiles", "바뀐 파일이 없습니다."));
    host.replaceChildren(quiet);
    return;
  }
  host.replaceChildren(...files.map(glFileNode));
}

/* 탭의 낱말: 수는 늦은 읽기가 도착했을 때만 실린다(실측: "파일(+수)"). */
function paintGlFilesTabWord() {
  const files = glItemReview?.files ?? null;
  say(el("gl-item-tab-files"), () => (files?.length
    ? t("task.gitlab.tabFilesCount", "파일 {{count}}", { count: files.length })
    : t("task.gitlab.tabFiles", "파일")));
}

async function loadGlMrReview() {
  const at = glItem;
  if (!at || at.kind !== "mr") return;
  const asking = glItemAsking;
  let review;
  try {
    review = await invoke("gitlab_mr_review", { project: at.project, number: at.number });
  } catch (error) {
    if (glItemAsking !== asking) return;
    sayGitlabItemTrouble(error, () =>
      t("task.gitlab.reviewFailed", "리뷰 정보를 불러오지 못했습니다"));
    return;
  }
  if (glItemAsking !== asking) return;
  glItemReview = review;
  paintGlFilesTabWord();
  paintGlItemFiles();
  paintGlReviewers();
}

function paintGlItemJobs() {
  const host = el("gl-item-jobs");
  if (glItemJobs === null) {
    host.replaceChildren();
    return;
  }
  if (glItemJobs.length === 0) {
    const quiet = document.createElement("p");
    quiet.className = "gl-item-quiet";
    say(quiet, () => t("task.gitlab.noJobs", "이 파이프라인에는 잡이 없습니다."));
    host.replaceChildren(quiet);
    return;
  }
  host.replaceChildren(...glItemJobs.map(glJobNode));
}

async function loadGlItemJobs() {
  const at = glItem;
  if (!at || at.pipeline == null) return;
  const asking = glItemAsking;
  let jobs;
  try {
    jobs = await invoke("gitlab_pipeline_jobs", {
      project: at.project,
      pipeline: at.pipeline,
    });
  } catch (error) {
    if (glItemAsking !== asking) return;
    sayGitlabItemTrouble(error, () =>
      t("task.gitlab.jobsFailed", "파이프라인을 불러오지 못했습니다"));
    return;
  }
  if (glItemAsking !== asking) return;
  glItemJobs = jobs;
  paintGlItemJobs();
}

async function toggleGlJobLog(job) {
  glJobOpen = glJobOpen === job.id ? null : job.id;
  paintGlItemJobs();
  if (glJobOpen !== job.id || glJobTraces.has(job.id)) return;
  const asking = glItemAsking;
  let text;
  try {
    text = await invoke("gitlab_job_trace", { project: glItem?.project, job: job.id });
  } catch (error) {
    if (glItemAsking !== asking) return;
    sayGitlabItemTrouble(error, () =>
      t("task.gitlab.jobsFailed", "파이프라인을 불러오지 못했습니다"));
    return;
  }
  if (glItemAsking !== asking) return;
  glJobTraces.set(job.id, text === "" ? t("task.gitlab.jobLogEmpty", "로그가 없습니다.") : text);
  paintGlItemJobs();
}

async function retryGlJob(job, button) {
  if (glItemBusy) return;
  glItemBusy = true;
  button.disabled = true;
  try {
    await invoke("gitlab_retry_job", { project: glItem?.project, job: job.id });
    // 다시 돈 잡은 새 잡이다 — 목록을 다시 물어 새 상태가 선다.
    glItemJobs = null;
    glJobTraces.delete(job.id);
    await loadGlItemJobs();
  } catch (error) {
    sayGitlabItemTrouble(error, () =>
      t("task.gitlab.retryFailed", "잡을 다시 돌리지 못했습니다"));
  } finally {
    glItemBusy = false;
    button.disabled = false;
  }
}

el("gl-item-shut").addEventListener("click", closeGitlabItem);
// 스크림의 빈 자리를 누르면 물러선다. Escape는 이 창의 겹 사다리가 맡는다 —
// 다이얼로그는 `dismissable`(팝오버의 판)이 아니라 그 목록에 한 칸으로 선다.
glItemScrim.addEventListener("mousedown", (event) => {
  if (event.target === glItemScrim) closeGitlabItem();
});

el("gl-item-open-web").addEventListener("click", () => {
  // 목록 행의 ↗와 같은 문으로 나간다.
  if (glItem) openExternal(glItem.url);
});

el("gl-item-start").addEventListener("click", () => {
  // 나가는 중인 요청이 있으면 이 카드는 움직이지 않는다 — 컴포저가 화면을
  // 가져가는 손짓이라, 카드만 남고 답이 도착할 자리가 없어진다.
  if (!glItem || glItemBusy) return;
  const item = glItem;
  closeGitlabItem();
  void startWorktreeFromProviderItem(item, "gitlab");
});

el("gl-item-comment-send").addEventListener("click", async () => {
  if (!glItem || glItemBusy) return;
  const field = el("gl-item-comment-field");
  if (!field.value.trim()) return;
  glItemBusy = true;
  const send = el("gl-item-comment-send");
  send.disabled = true;
  try {
    await invoke("gitlab_comment_item", {
      project: glItem.project,
      kind: glItem.kind,
      number: glItem.number,
      body: field.value,
    });
    field.value = "";
    // 성공한 뒤 다시 읽는다: 방금 쓴 말이 대화에 서지 않으면, 사람은 그것이
    // 갔는지를 이 창에서 알 방법이 없다.
    await readGitlabItem();
  } catch (error) {
    sayGitlabItemTrouble(error, () =>
      t("task.gitlab.commentFailed", "댓글을 보내지 못했습니다"));
  } finally {
    glItemBusy = false;
    send.disabled = false;
  }
});

el("gl-item-state-act").addEventListener("click", async () => {
  if (!glItem || glItemBusy) return;
  glItemBusy = true;
  const act = el("gl-item-state-act");
  act.disabled = true;
  try {
    await invoke("gitlab_set_item_open", {
      project: glItem.project,
      kind: glItem.kind,
      number: glItem.number,
      open: act.dataset.wantOpen === "true",
    });
    await readGitlabItem();
    // 목록의 그 행도 새 상태를 입어야 한다.
    void loadTaskGitlab();
  } catch (error) {
    sayGitlabItemTrouble(error, () =>
      t("task.gitlab.stateFailed", "상태를 바꾸지 못했습니다"));
  } finally {
    glItemBusy = false;
    act.disabled = false;
  }
});

el("gl-item-edit").addEventListener("click", () => {
  if (!glItem) return;
  // 폼은 열 때마다 카드가 아는 원문으로 다시 심긴다 — 지난 편집의 잔상이
  // 남으면 사람이 고치지 않은 것이 고쳐진 것처럼 나간다.
  el("gl-item-edit-title").value = glItem.told?.title ?? "";
  el("gl-item-edit-body").value = glItem.told?.body ?? "";
  el("gl-item-edit-labels").value = (glItem.told?.labels ?? []).join(", ");
  glItemEditing = true;
  paintGlItemEdit();
});

el("gl-item-edit-cancel").addEventListener("click", () => {
  glItemEditing = false;
  paintGlItemEdit();
});

/* 쉼표 목록을 이름들로 — 빈 조각은 버린다("bug,, backend"의 그 쉼표). */
function parseGlLabels(text) {
  return text.split(",").map((name) => name.trim()).filter(Boolean);
}

el("gl-item-edit-save").addEventListener("click", async () => {
  if (!glItem || glItemBusy) return;
  const title = el("gl-item-edit-title").value;
  const body = el("gl-item-edit-body").value;
  // 라벨은 통째로 바꿔치지 않는다 — 실측의 add/remove 두 갈래로, 지금 것과
  // 적힌 것의 차만 나간다. 순서만 바뀐 목록은 그래서 아무것도 보내지 않는다.
  const wanted = parseGlLabels(el("gl-item-edit-labels").value);
  const had = glItem.told?.labels ?? [];
  const adds = wanted.filter((name) => !had.includes(name));
  const drops = had.filter((name) => !wanted.includes(name));
  const ask = {
    project: glItem.project,
    number: glItem.number,
    title: title !== glItem.told?.title ? title : null,
    body: body !== glItem.told?.body ? body : null,
    addLabels: adds.length ? adds.join(",") : null,
    removeLabels: drops.length ? drops.join(",") : null,
  };
  const quiet = [ask.title, ask.body, ask.addLabels, ask.removeLabels]
    .every((field) => field == null);
  if (quiet) {
    // 바뀐 것이 없으면 보낼 것도 없다 — 폼만 접는다.
    glItemEditing = false;
    paintGlItemEdit();
    return;
  }
  glItemBusy = true;
  const save = el("gl-item-edit-save");
  save.disabled = true;
  try {
    await invoke("gitlab_update_mr", ask);
    // 저장된 말이 카드에 서지 않으면 사람은 그것이 갔는지 모른다(댓글의 그
    // 규칙). 제목은 목록의 그 행에도 서 있다.
    await readGitlabItem();
    void loadTaskGitlab();
  } catch (error) {
    sayGitlabItemTrouble(error, () =>
      t("task.gitlab.updateFailed", "변경을 저장하지 못했습니다"));
  } finally {
    glItemBusy = false;
    save.disabled = false;
  }
});

/* 실측 deriveMergeable(out/main/index.js:68864): GitLab의 세 필드가 한 답으로
 * 접힌다 — has_conflicts가 먼저 말하고, detailed_merge_status가 다음이며,
 * 그 필드가 아예 없는 낡은 서버만 merge_status로 답한다. */
function glMergeable(told) {
  if (told?.has_conflicts === true) return "conflicting";
  const detailed = told?.detailed_merge_status;
  if (detailed === "mergeable") return "mergeable";
  if (detailed === "broken_status" || detailed === "conflict") return "conflicting";
  if (detailed === undefined && told?.merge_status === "can_be_merged") return "mergeable";
  return "unknown";
}

/* 실측 classifyPipelineString(:68930) — 파이프라인의 낱말 셋. */
function glPipelineWord(status) {
  const word = String(status ?? "").toLowerCase();
  if (word === "success") return "success";
  if (word === "failed" || word === "action_required") return "failure";
  if (["manual", "created", "pending", "running", "waiting_for_resource", "preparing", "scheduled"]
    .includes(word)) return "pending";
  return "neutral";
}

/* 실측 presentGitLabMRMergeState의 사다리 그대로: 끝난 상태 → 충돌 → 게이트
 * 코드 → "병합은 되지만 검사가"의 두 경고 → 병합 가능. {said, can}. */
function glMergeGate(told) {
  if (told?.state === "merged") return { said: t("task.gitlab.gate.merged", "병합됨"), can: false };
  if (told?.state === "closed") return { said: t("task.gitlab.gate.closed", "닫힘"), can: false };
  if (told?.draft === true || told?.work_in_progress === true) {
    return { said: t("task.gitlab.gate.draft", "초안"), can: false };
  }
  const pipeline = glPipelineWord(told?.head_pipeline?.status ?? told?.pipeline?.status);
  const mergeable = glMergeable(told);
  if (mergeable === "conflicting") {
    return { said: t("task.gitlab.gate.conflicts", "충돌"), can: false };
  }
  if (mergeable !== "mergeable") {
    const code = String(told?.detailed_merge_status ?? "").toLowerCase();
    const gated = {
      not_approved: t("task.gitlab.gate.approval", "승인 필요"),
      requested_changes: t("task.gitlab.gate.changes", "변경 요청됨"),
      discussions_not_resolved: t("task.gitlab.gate.threads", "미해결 스레드"),
      ci_must_pass: t("task.gitlab.gate.ciMust", "검사 통과 필요"),
      ci_still_running: t("task.gitlab.gate.ciPending", "검사 대기 중"),
      need_rebase: t("task.gitlab.gate.behind", "뒤처짐"),
      draft_status: t("task.gitlab.gate.draft", "초안"),
    }[code];
    if (gated) return { said: gated, can: false };
    if ([
      "blocked_status", "policies_denied", "external_status_checks",
      "security_policy_violations", "locked_paths", "locked_lfs_files",
      "jira_association_missing", "title_regex", "not_open",
    ].includes(code)) {
      return { said: t("task.gitlab.gate.blocked", "차단됨"), can: false };
    }
    if (!["checking", "unchecked", "preparing"].includes(code) && pipeline === "failure") {
      return { said: t("task.gitlab.gate.ciFailed", "검사 실패"), can: false };
    }
    return { said: t("task.gitlab.gate.checking", "확인 중"), can: false };
  }
  if (pipeline === "failure") return { said: t("task.gitlab.gate.ciFailed", "검사 실패"), can: true };
  if (pipeline === "pending") return { said: t("task.gitlab.gate.ciPending", "검사 대기 중"), can: true };
  return { said: t("task.gitlab.gate.able", "병합 가능"), can: true };
}

function closeGlMergeMenu() {
  closing(el("gl-merge-menu"));
}

/* 병합의 세 길(실측: merge·squash·rebase)이 한 팝오버에 선다 — 링크 메뉴의
 * 그 판이다. 방식 낱말은 창을 넘어 그대로 가고, 문 밖은 `MergeMethod`가
 * 지킨다. */
el("gl-item-merge").addEventListener("click", () => {
  if (!glItem || glItemBusy) return;
  const pop = el("gl-merge-menu");
  const host = el("gl-merge-menu-body");
  host.replaceChildren();
  // GitLab의 답이 문 위에 선다(실측 ChecksPanel): 사유 한 줄, 그리고 사유가
  // 잠그면 세 길 모두 잠긴다.
  const gate = glMergeGate(glItem.told);
  const word = document.createElement("p");
  word.className = "gl-merge-gate";
  word.dataset.tone = gate.can ? "ready" : "halt";
  say(word, () => gate.said);
  host.appendChild(word);
  menuRow(host, {
    said: t("task.gitlab.merge", "병합"),
    disabled: !gate.can,
    close: closeGlMergeMenu,
    run: () => void mergeGlItem("merge"),
  });
  menuRow(host, {
    said: t("task.gitlab.mergeSquash", "스쿼시 병합"),
    disabled: !gate.can,
    close: closeGlMergeMenu,
    run: () => void mergeGlItem("squash"),
  });
  menuRow(host, {
    said: t("task.gitlab.mergeRebase", "리베이스 병합"),
    disabled: !gate.can,
    close: closeGlMergeMenu,
    run: () => void mergeGlItem("rebase"),
  });
  showing(pop);
  const at = el("gl-item-merge").getBoundingClientRect();
  const wide = pop.offsetWidth;
  const tall = pop.offsetHeight;
  pop.style.left = `${Math.max(8, Math.min(at.left, window.innerWidth - wide - 8))}px`;
  pop.style.top = `${Math.max(8, Math.min(at.bottom + 6, window.innerHeight - tall - 8))}px`;
});

dismissable(el("gl-merge-menu"), closeGlMergeMenu);

/* 이 카드 자신의 거절 — `glab`이 아니라 이 창이 세운 문장이 서는 자리다. */
function sayGlInlineRefusal(word) {
  const line = el("gl-item-error");
  line.hidden = false;
  say(line, word);
}

el("gl-inline-send").addEventListener("click", async () => {
  if (!glItem || glItemBusy) return;
  const picked = el("gl-inline-file").value;
  const line = Number.parseInt(el("gl-inline-line").value, 10);
  const body = el("gl-inline-body").value;
  const file = (glItemReview?.files ?? []).find((one) => one.path === picked);
  // 셋 다 있어야 한 자리다(실측의 그 거절문).
  if (!file || !Number.isFinite(line) || line <= 0 || !body.trim()) {
    sayGlInlineRefusal(() =>
      t("task.gitlab.inlineNeedAll", "파일, 줄, 내용이 모두 필요합니다"));
    return;
  }
  // 세 SHA가 없으면 자리를 말할 수 없다 — 보내기 전에 이 창이 거절한다.
  const refs = glItem.told?.diff_refs;
  if (!refs) {
    sayGlInlineRefusal(() =>
      t("task.gitlab.inlineNoRefs", "diff 기준이 없어 인라인 댓글을 보낼 수 없습니다"));
    return;
  }
  glItemBusy = true;
  const send = el("gl-inline-send");
  send.disabled = true;
  try {
    await invoke("gitlab_inline_comment", {
      project: glItem.project,
      number: glItem.number,
      place: {
        path: file.path,
        old_path: file.old_path ?? null,
        line,
        refs,
      },
      body,
    });
    el("gl-inline-body").value = "";
    // 대화에 새 목소리가 섰다 — 카드를 다시 읽어 그 자리를 보인다.
    await readGitlabItem();
  } catch (error) {
    sayGitlabItemTrouble(error, () =>
      t("task.gitlab.inlineFailed", "인라인 댓글을 보내지 못했습니다"));
  } finally {
    glItemBusy = false;
    send.disabled = false;
  }
});

el("gl-item-reviewers-manage").addEventListener("click", async () => {
  if (!glItem || glItemMembers !== null) return;
  const manage = el("gl-item-reviewers-manage");
  manage.disabled = true;
  const asking = glItemAsking;
  let bench;
  try {
    bench = await invoke("gitlab_project_members", { project: glItem.project });
  } catch (error) {
    if (glItemAsking === asking) {
      sayGitlabItemTrouble(error, () =>
        t("task.gitlab.membersFailed", "구성원을 불러오지 못했습니다"));
    }
    manage.disabled = false;
    return;
  }
  manage.disabled = false;
  if (glItemAsking !== asking) return;
  glItemMembers = bench;
  paintGlReviewerBench();
});

el("gl-item-reviewer-put").addEventListener("click", () => {
  const picked = Number(el("gl-item-reviewer-pick").value);
  if (!picked) return;
  const seated = (glItem?.told?.reviewers ?? []).map((one) => one.id);
  void setGlReviewers([...seated, picked]);
});

/* 통째로 바꾼다(실측의 그 PUT) — 성공 뒤 카드를 다시 읽으면 칩도 승인 줄도
 * 그 답에서 다시 선다. */
async function setGlReviewers(ids) {
  if (!glItem || glItemBusy) return;
  glItemBusy = true;
  try {
    await invoke("gitlab_set_mr_reviewers", {
      project: glItem.project,
      number: glItem.number,
      reviewers: ids,
    });
    glItemMembers = null;
    await readGitlabItem();
  } catch (error) {
    sayGitlabItemTrouble(error, () =>
      t("task.gitlab.reviewersFailed", "리뷰어를 바꾸지 못했습니다"));
  } finally {
    glItemBusy = false;
  }
}

async function mergeGlItem(method) {
  if (!glItem || glItemBusy) return;
  glItemBusy = true;
  const button = el("gl-item-merge");
  button.disabled = true;
  try {
    await invoke("gitlab_merge_mr", {
      project: glItem.project,
      number: glItem.number,
      method,
    });
    // 병합된 카드는 `merged`를 입고 이 문을 접는다 — 목록의 그 행도 같이.
    await readGitlabItem();
    void loadTaskGitlab();
  } catch (error) {
    sayGitlabItemTrouble(error, () =>
      t("task.gitlab.mergeFailed", "병합하지 못했습니다"));
  } finally {
    glItemBusy = false;
    button.disabled = false;
  }
}

function setTaskSource(source, { chosen = true } = {}) {
  // 탭·지름길·시험이 부르는 것은 고르는 손이고, 감춤이 부르는 것은 물러서는
  // 손이다. 후자는 「고른 것」을 건드리지 않는다.
  if (chosen) chosenTaskSource = source;
  taskSource = source;
  for (const name of TASK_SOURCES) {
    const tab = el(`task-tab-${name}`);
    const active = name === source;
    tab.classList.toggle("is-active", active);
    tab.setAttribute("aria-selected", String(active));
    el(`task-panel-${name}`).hidden = !active;
  }
  paintTaskContext();
  // GitHub 판은 서는 순간 진단부터 다시 그린다 — Jira의 상태는 setTaskOpen이
  // 이미 데워 두지만, gh의 답은 이 판만 쓴다. GitLab도 같은 사정이다.
  if (source === "github" && !taskView.hidden) void paintGithubPanel();
  if (source === "gitlab" && !taskView.hidden) void paintGitlabPanel();
  if (source === "linear" && !taskView.hidden) void paintLinearPanel();
}

for (const name of TASK_SOURCES) {
  el(`task-tab-${name}`).addEventListener("click", () => setTaskSource(name));
}

el("task-close").addEventListener("click", () => setTaskOpen(false));


function paintCrashSettings(limits) {
  if (!limits) return;
  el("crash-watchdog").checked = limits.watchdog;
  const first = el("crash-first-ms");
  const second = el("crash-second-ms");
  first.min = limits.ping_ms;
  first.max = limits.threshold_max_ms - limits.ping_ms;
  first.step = limits.ping_ms;
  first.value = limits.first_ms;
  second.min = limits.first_ms + limits.ping_ms;
  second.max = limits.threshold_max_ms;
  second.step = limits.ping_ms;
  second.value = limits.second_ms;
  first.disabled = second.disabled = !limits.watchdog;
}

el("crash-watchdog").addEventListener("change", () => {
  void commitSetting("crash.watchdog", "set_crash_watchdog", { patch: { watchdog: el("crash-watchdog").checked } });
});
for (const [id, field] of [["crash-first-ms", "first_ms"], ["crash-second-ms", "second_ms"]]) {
  el(id).addEventListener("change", () => {
    const value = Number(el(id).value);
    if (el(id).value.trim() === "" || !Number.isSafeInteger(value) || value < 0) {
      void refreshSettingsSnapshot();
      return;
    }
    void commitSetting(`crash.${field}`, "set_crash_watchdog", { patch: { [field]: value } });
  });
}
