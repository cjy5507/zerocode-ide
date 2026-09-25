use super::*;

pub(super) const SETTINGS_DOCUMENT_FILE: &str = settings_file::PREFERENCES;
pub(super) const PROJECT_SETTINGS_DOCUMENT_FILE: &str = settings_file::PROJECT;
pub(super) const LEGACY_MIGRATION_LEDGER_FILE: &str = settings_file::LEGACY_MIGRATIONS;
pub(super) const SETTINGS_SCHEMA_VERSION: u32 = 1;
pub(super) const SETTINGS_CHANGED_EVENT: &str = "settings:changed";
/// One connection's state moved. Every window hears it; the payload is the
/// whole [`ssh_link::LinkReport`], so no listener has to ask again.
pub(super) const SSH_LINK_EVENT: &str = "ssh:link-changed";
pub(super) const TERMINAL_CLIPBOARD_WRITE_EVENT: &str = "terminal:clipboard-write";
pub(super) const MAIN_WINDOW_LABEL: &str = "main";

/// Each raw settings family may be consumed once, independently.
///
/// The stable string is persisted instead of an enum ordinal so adding a new
/// migration cannot change the meaning of an older ledger.
#[derive(Clone, Copy)]
pub(super) enum LegacyMigration {
    SettingsDocument,
    ProjectSettings,
    AgentLaunch,
}

impl LegacyMigration {
    pub(super) const fn key(self) -> &'static str {
        match self {
            Self::SettingsDocument => "settings-document-v1",
            Self::ProjectSettings => "project-settings-v1",
            Self::AgentLaunch => "agent-launch-v1",
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
pub(super) struct LegacyMigrationLedger {
    #[serde(default)]
    pub(super) completed: BTreeSet<String>,
}

pub(super) fn legacy_migration_completed(
    repository: &settings::SettingsRepository,
    migration: LegacyMigration,
) -> Result<bool, String> {
    Ok(repository
        .read_json::<LegacyMigrationLedger>(LEGACY_MIGRATION_LEDGER_FILE)
        .map_err(|error| error.to_string())?
        .value
        .unwrap_or_default()
        .completed
        .contains(migration.key()))
}

/// Durably record that a raw settings family has been consumed.
///
/// This goes through the same locked, atomic repository transaction as the
/// canonical documents. The raw files remain untouched as rollback evidence;
/// the ledger is what distinguishes first install from an intentional reset.
pub(super) fn complete_legacy_migration(
    repository: &settings::SettingsRepository,
    migration: LegacyMigration,
) -> Result<(), String> {
    if legacy_migration_completed(repository, migration)? {
        return Ok(());
    }
    repository
        .mutate_json(
            LEGACY_MIGRATION_LEDGER_FILE,
            LegacyMigrationLedger::default,
            |ledger| {
                ledger.completed.insert(migration.key().to_string());
            },
        )
        .map(|_| ())
        .map_err(|error| error.to_string())
}

pub(super) mod setting_key {
    pub const CRASH: &str = "crash";
    pub const THEME: &str = "theme";
    pub const LOCALE: &str = "locale";
    pub const UI_ZOOM_LEVEL: &str = "ui_zoom_level";
    pub const APP_FONT_FAMILY: &str = "app_font_family";
    pub const EDITING_PREFS: &str = "editing_prefs";
    pub const UPDATE: &str = "update";
    pub const TERMINAL_PREFS: &str = "terminal_prefs";
    pub const TERMINAL_COMMAND: &str = "terminal_command";
    pub const SETUP_SCRIPT_LAUNCH_MODE: &str = "setup_script_launch_mode";
    pub const TERMINAL_SHORTCUT_POLICY: &str = "terminal_shortcut_policy";
    pub const WINDOW_MATERIAL: &str = "window_material";
    pub const DEFAULT_AGENT: &str = "default_agent";
    pub const AGENT_TEAMS_MODE: &str = "agent_teams_mode";
    /// `accounts.claudeAutoSwitch` in the briefing's words: whether the
    /// window may move the Claude account by itself (t-7538).
    pub const CLAUDE_AUTOSWITCH_MODE: &str = "claude_autoswitch_mode";
    pub const HIDDEN_SHORTCUTS: &str = "hidden_shortcuts";
    pub const KEYBINDINGS: &str = "keybindings";
    pub const HIDDEN_TASK_SOURCES: &str = "hidden_task_sources";
    pub const DEFAULT_TASK_SOURCE: &str = "default_task_source";
    pub const HIDE_AUTOMATION_WORKSPACES: &str = "hide_automation_workspaces";
    pub const HIDE_DEFAULT_BRANCH_WORKSPACES: &str = "hide_default_branch_workspaces";
    pub const HIDE_DETACHED_HEAD_WORKSPACES: &str = "hide_detached_head_workspaces";
    pub const SIDEBAR_VIEW: &str = "sidebar_view";
    pub const HIDE_SLEEPING_WORKSPACES: &str = "hide_sleeping_workspaces";
    pub const KEEP_DEFAULT_BRANCH_AWAKE: &str = "keep_default_branch_awake";
    pub const HIDE_AGENT_SCRATCH_WORKSPACES: &str = "hide_agent_scratch_workspaces";
    pub const WORKTREE_CARD_PROPERTIES: &str = "worktree_card_properties";
    pub const WORKSPACE_BOARD: &str = "workspace_board";
    pub const COMPUTER_AWAKE_MODE: &str = "computer_awake_mode";
    pub const COMPUTER_CONFIRM_PAYMENT: &str = "computer_confirm_payment";
    pub const COMPUTER_CONFIRM_TRANSFER: &str = "computer_confirm_transfer";
    pub const COMPUTER_CONFIRM_DELETE: &str = "computer_confirm_delete";
    pub const COMPUTER_LIVE_REFLEX: &str = "computer_live_reflex";
    pub const OPENCODE_COOKIE_CONFIGURED: &str = "opencode_cookie_configured";
    pub const OPENCODE_WORKSPACE: &str = "opencode_workspace";
    pub const CONFIRM_CLOSE_PINNED: &str = "confirm_close_pinned";
    pub const FLOATING_WORKSPACE: &str = "floating_workspace";
    pub const SKIP_CLOSE_TERMINAL_WITH_RUNNING_PROCESS_CONFIRM: &str =
        "skip_close_terminal_with_running_process_confirm";
    pub const CTRL_TAB_ORDER_MODE: &str = "ctrl_tab_order_mode";
    pub const WORKSPACE_CREATION_PREFS: &str = "workspace_creation_prefs";
    pub const OPEN_IN_APPLICATIONS: &str = "open_in_applications";
    pub const SKIP_DELETE_WORKTREE_CONFIRM: &str = "skip_delete_worktree_confirm";
    pub const SKIP_DELETE_AUTOMATION_CONFIRM: &str = "skip_delete_automation_confirm";
    pub const VAULT_SESSION_LIMIT: &str = "vault.sessionLimit";
    pub const ARTIFACTS_RETENTION_DAYS: &str = "artifacts_retention_days";
    pub const DIFF_SIDE_BY_SIDE: &str = "diff_side_by_side";
    pub const CONVERSATION_FOCUS_VIEW: &str = "conversation_focus_view";
    pub const PANEL_WIDTHS: &str = "panel_widths";
    pub const WORKTREE_PREFS: &str = "worktree_prefs";
    pub const NOTIFICATIONS: &str = "notifications";
    pub const BROWSER: &str = "browser";
    pub const STATUS_BAR_ITEMS: &str = "status_bar_items";
    pub const USAGE_PERCENTAGE_DISPLAY: &str = "usage_percentage_display";
    pub const STATUS_BAR_USAGE_MODE: &str = "status_bar_usage_mode";
    pub const USAGE_ANALYTICS_OFF: &str = "usage_analytics_off";
    pub const SOURCE_CONTROL_VIEW_MODE: &str = "source_control_view_mode";
    pub const SHOW_TITLEBAR_APP_NAME: &str = "show_titlebar_app_name";
    pub const SHOW_MENU_BAR_ICON: &str = "show_menu_bar_icon";
    pub const MINIMIZE_TO_TRAY_ON_CLOSE: &str = "minimize_to_tray_on_close";
    pub const COMPACT_WORKTREE_CARDS: &str = "compact_worktree_cards";
    pub const AGENT_ACTIVITY_DISPLAY: &str = "agent_activity_display";
    pub const SHOW_GIT_IGNORED_FILES: &str = "show_git_ignored_files";
    pub const SOURCE_CONTROL_GROUP_ORDER: &str = "source_control_group_order";
    pub const SOURCE_CONTROL_COMPARE_BASE: &str = "source_control_compare_base";
    pub const REFRESH_LOCAL_BASE_REF_ON_WORKTREE_CREATE: &str =
        "refresh_local_base_ref_on_worktree_create";
    pub const LEFT_SIDEBAR_APPEARANCE_MODE: &str = "left_sidebar_appearance_mode";
    pub const LEFT_SIDEBAR_TINT_COLOR: &str = "left_sidebar_tint_color";
    pub const LEFT_SIDEBAR_TINT_OPACITY: &str = "left_sidebar_tint_opacity";
    pub const SECOND_BRAIN_VAULT: &str = "second_brain_vault";
    pub const SECOND_BRAIN_WEEKLY_REVIEW: &str = "second_brain_weekly_review";
    pub const SECOND_BRAIN_SCENES: &str = "second_brain_scenes";
    pub const SECOND_BRAIN_EXPLORE: &str = "second_brain_explore";
    pub const GOOGLE_ACCOUNT_EMAIL: &str = "google_account_email";
}

/// Status fields ZeroCode can actually render, in Orca's own default
/// order (`status-bar-defaults.ts:3-15` — claude, codex, antigravity,
/// … resource-usage, ports; every one of them ON out of the
/// box). The five names between antigravity and resource-usage there —
/// opencode-go, kimi, minimax, grok, ssh — arrive here the day their
/// segments do: a settings switch that can never paint a segment is not
/// an available item. Caffeinate is absent on purpose, not for lack of a
/// segment: Orca draws it OUTSIDE this catalog, always on with no
/// checkbox (`StatusBar.tsx:2393`, and none in :2456-2571's list).
///
/// This list IS the `set_status_bar_item` gate, so it must move in step
/// with the window's `STATUS_BAR_ITEM_CATALOG` — it did not when the
/// Antigravity gauge landed, and its toggle was being refused
/// by the very backend that painted them (caught 2026-08-18 while seating
/// caffeinate).
pub(super) const STATUS_BAR_ITEMS: &[&str] = &[
    "claude",
    "codex",
    "antigravity",
    "kimi",
    "grok",
    "opencode-go",
    "resource-usage",
    "ports",
];
pub(super) const DEFAULT_APP_FONT_FAMILY: &str = "Geist";

pub(super) fn default_status_bar_items() -> Vec<String> {
    STATUS_BAR_ITEMS
        .iter()
        .map(|item| (*item).to_string())
        .collect()
}

pub(super) fn normalize_status_bar_items(items: Vec<String>) -> Vec<String> {
    STATUS_BAR_ITEMS
        .iter()
        .filter(|candidate| items.iter().any(|item| item == **candidate))
        .map(|item| (*item).to_string())
        .collect()
}

/// Turn on every catalog item this document has never been offered.
///
/// [`normalize_status_bar_items`] intersects, which means a name missing from
/// a stored list is indistinguishable from one the person switched off — and
/// so an item that ships AFTER a list was written can never appear. That is
/// not hypothetical: four gauges (antigravity, kimi, grok,
/// opencode-go) joined this catalog after most stored lists were written, and
/// on those installs their settings checkboxes read unchecked, ticking them
/// did nothing anybody could see, and no road existed that would ever add
/// them. "Antigravity never shows up" was this, not the OAuth gate below it.
///
/// So the document remembers what it was OFFERED, separately from what is on.
/// A name in the catalog but not in `seen` has never been put in front of
/// this person: it arrives ON, which is how every one of them shipped
/// (`status-bar-defaults.ts:3-15` — all of them on out of the box). A name in
/// `seen` and not in the list was switched off deliberately and stays off
/// forever.
///
/// The first run under this code has an empty `seen`, so every catalog item
/// counts as new and comes on once. That is deliberate and the cost is
/// bounded: a gauge with nothing behind it hides itself
/// (`status == "unavailable"`), so the only visible effect is on a machine
/// that genuinely has that provider configured — which is the machine that
/// wanted the gauge.
pub(super) fn adopt_unseen_status_bar_items(items: &mut Vec<String>, seen: &mut Vec<String>) {
    for name in STATUS_BAR_ITEMS {
        if seen.iter().any(|held| held == name) {
            continue;
        }
        if !items.iter().any(|held| held == name) {
            items.push((*name).to_string());
        }
    }
    *items = normalize_status_bar_items(std::mem::take(items));
    *seen = STATUS_BAR_ITEMS
        .iter()
        .map(|name| (*name).to_string())
        .collect();
}

/// Which side of a provider quota is drawn. Orca uses the same value for the
/// status-bar figure, its fill width, and the detailed usage rows.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum UsagePercentageDisplay {
    #[default]
    Used,
    Remaining,
}

/// How much provider quota detail the status bar and its own usage popover
/// show. Orca defaults to the complete roster and offers the tightest window
/// as the one deliberately condensed alternative.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StatusBarUsageMode {
    #[default]
    Verbose,
    Compact,
}

/// Whether the changed files stand as a flat list or as their directory tree.
///
/// Orca's `sourceControlViewMode` (`ui-chrome-types`), and a SETTING rather
/// than window state for the reason every view mode here is one: a person who
/// chose the tree chose it about their working habit, not about this launch.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum SourceControlViewMode {
    #[default]
    List,
    Tree,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum BrowserSearchEngine {
    #[default]
    Google,
    Duckduckgo,
    Bing,
    Kagi,
}

/// The order shown while Control+Tab is held. `Mru` follows the tab visit
/// history already owned by each pane; `Sequential` follows the visible strip.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum CtrlTabOrderMode {
    #[default]
    Mru,
    Sequential,
}

/// 워크스페이스 카드가 그릴 줄 아는 것들, 카드가 그리는 순서로.
///
/// 백엔드가 이 목록을 아는 이유는 하나다: 창이 보낸 낱말이 이 창이 아는 것인지
/// 여기서 판정해야, 모르는 낱말이 설정 파일에 조용히 눌러앉지 않는다.
pub(super) const WORKTREE_CARD_PROPERTIES: [&str; 4] =
    ["branch", "ports", "agents", "default-badge"];

pub(super) fn default_worktree_card_properties() -> Vec<String> {
    WORKTREE_CARD_PROPERTIES
        .iter()
        .map(|held| (*held).to_string())
        .collect()
}

/// 아는 낱말만, 카드가 그리는 순서로. 저장된 목록의 순서가 아니라 이 순서를
/// 쓰는 것은 두 창이 서로 다른 순서로 같은 목록을 적어 두지 않게 하기 위해서다.
pub(super) fn normalize_worktree_card_properties(held: &[String]) -> Vec<String> {
    WORKTREE_CARD_PROPERTIES
        .iter()
        .filter(|known| held.iter().any(|one| one == *known))
        .map(|known| (*known).to_string())
        .collect()
}

/// One user-defined lane in the workspace board.
///
/// These are deliberately data rather than an enum: the four defaults are a
/// useful starting workflow, not the only workflow the application permits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct WorkspaceBoardStatus {
    #[serde(default)]
    pub(super) id: String,
    #[serde(default)]
    pub(super) label: String,
    #[serde(default)]
    pub(super) color: String,
    #[serde(default)]
    pub(super) icon: String,
}

/// The board-only metadata for one catalogued workspace.
///
/// The path is the key in [`WorkspaceBoardSettings::cards`]. An absent status
/// means the current default, so changing the set of lanes cannot strand an
/// old checkout in a lane that no longer exists.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct WorkspaceBoardCard {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(super) status: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) pinned: bool,
}

pub(super) const WORKSPACE_BOARD_COLUMN_WIDTH_DEFAULT: u32 = 308;
pub(super) const WORKSPACE_BOARD_COLUMN_WIDTH_MIN: u32 = 220;
pub(super) const WORKSPACE_BOARD_COLUMN_WIDTH_MAX: u32 = 520;
pub(super) const WORKSPACE_BOARD_STATUS_LABEL_MAX: usize = 32;
pub(super) const WORKSPACE_BOARD_STATUS_COUNT_MAX: usize = 32;
pub(super) const WORKSPACE_BOARD_CARD_COUNT_MAX: usize = 4096;

pub(super) const WORKSPACE_BOARD_COLORS: [&str; 11] = [
    "neutral",
    "blue",
    "sky",
    "violet",
    "amber",
    "emerald",
    "rose",
    "zinc",
    "conductor-done",
    "conductor-review",
    "conductor-progress",
];

pub(super) const WORKSPACE_BOARD_ICONS: [&str; 16] = [
    "circle",
    "circle-dot",
    "circle-progress",
    "circle-dashed",
    "circle-ellipsis",
    "git-pull-request",
    "timer",
    "flag",
    "circle-alert",
    "circle-pause",
    "circle-play",
    "circle-check",
    "ban",
    "conductor-done",
    "conductor-review",
    "conductor-progress",
];

pub(super) fn default_workspace_board_statuses() -> Vec<WorkspaceBoardStatus> {
    [
        ("todo", "Todo", "neutral", "circle"),
        (
            "in-progress",
            "In progress",
            "conductor-progress",
            "conductor-progress",
        ),
        (
            "in-review",
            "In review",
            "conductor-review",
            "conductor-review",
        ),
        ("completed", "Done", "conductor-done", "conductor-done"),
    ]
    .into_iter()
    .map(|(id, label, color, icon)| WorkspaceBoardStatus {
        id: id.to_string(),
        label: label.to_string(),
        color: color.to_string(),
        icon: icon.to_string(),
    })
    .collect()
}

pub(super) fn workspace_board_default_visual(id: &str) -> Option<(&'static str, &'static str)> {
    match id {
        "todo" => Some(("neutral", "circle")),
        "in-progress" => Some(("conductor-progress", "conductor-progress")),
        "in-review" => Some(("conductor-review", "conductor-review")),
        "completed" => Some(("conductor-done", "conductor-done")),
        _ => None,
    }
}

pub(super) fn workspace_board_label(raw: &str, fallback: &str) -> String {
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let chosen = if normalized.is_empty() {
        fallback
    } else {
        &normalized
    };
    chosen
        .chars()
        .take(WORKSPACE_BOARD_STATUS_LABEL_MAX)
        .collect()
}

pub(super) fn workspace_board_status_id(raw: &str, fallback: &str) -> String {
    let source = if raw.trim().is_empty() { fallback } else { raw };
    let mut id = String::new();
    let mut separated = false;
    for character in source.trim().to_ascii_lowercase().chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            id.push(character);
            separated = false;
        } else if !separated && !id.is_empty() {
            id.push('-');
            separated = true;
        }
    }
    while id.ends_with('-') {
        id.pop();
    }
    if id.is_empty() {
        "status".to_string()
    } else {
        id
    }
}

pub(super) fn unused_workspace_board_status_id(base: &str, used: &BTreeSet<String>) -> String {
    if !used.contains(base) {
        return base.to_string();
    }
    for index in 2..100 {
        let candidate = format!("{base}-{index}");
        if !used.contains(&candidate) {
            return candidate;
        }
    }
    format!("status-{}", used.len() + 1)
}

pub(super) fn normalized_workspace_board_statuses(
    statuses: Vec<WorkspaceBoardStatus>,
) -> Vec<WorkspaceBoardStatus> {
    let mut normalized = Vec::new();
    let mut used = BTreeSet::new();
    for (index, status) in statuses
        .into_iter()
        .take(WORKSPACE_BOARD_STATUS_COUNT_MAX)
        .enumerate()
    {
        let fallback = format!("Status {}", index + 1);
        let label = workspace_board_label(&status.label, &fallback);
        let proposed = workspace_board_status_id(&status.id, &label);
        let id = unused_workspace_board_status_id(&proposed, &used);
        used.insert(id.clone());
        let default = workspace_board_default_visual(&id);
        let color = WORKSPACE_BOARD_COLORS
            .contains(&status.color.as_str())
            .then_some(status.color)
            .or_else(|| default.map(|(color, _)| color.to_string()))
            .unwrap_or_else(|| {
                WORKSPACE_BOARD_COLORS[index % WORKSPACE_BOARD_COLORS.len()].to_string()
            });
        let icon = WORKSPACE_BOARD_ICONS
            .contains(&status.icon.as_str())
            .then_some(status.icon)
            .or_else(|| default.map(|(_, icon)| icon.to_string()))
            .unwrap_or_else(|| "circle-dot".to_string());
        normalized.push(WorkspaceBoardStatus {
            id,
            label,
            color,
            icon,
        });
    }
    if normalized.is_empty() {
        default_workspace_board_statuses()
    } else {
        normalized
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct WorkspaceBoardSettings {
    #[serde(default = "default_workspace_board_statuses")]
    pub(super) statuses: Vec<WorkspaceBoardStatus>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) cards: BTreeMap<String, WorkspaceBoardCard>,
    #[serde(default = "default_workspace_board_column_width")]
    pub(super) column_width: u32,
}

pub(super) const fn default_workspace_board_column_width() -> u32 {
    WORKSPACE_BOARD_COLUMN_WIDTH_DEFAULT
}

impl Default for WorkspaceBoardSettings {
    fn default() -> Self {
        Self {
            statuses: default_workspace_board_statuses(),
            cards: BTreeMap::new(),
            column_width: default_workspace_board_column_width(),
        }
    }
}

impl WorkspaceBoardSettings {
    pub(super) fn normalized(mut self) -> Self {
        self.statuses = normalized_workspace_board_statuses(self.statuses);
        self.column_width = self.column_width.clamp(
            WORKSPACE_BOARD_COLUMN_WIDTH_MIN,
            WORKSPACE_BOARD_COLUMN_WIDTH_MAX,
        );
        let known: BTreeSet<_> = self
            .statuses
            .iter()
            .map(|status| status.id.as_str())
            .collect();
        self.cards.retain(|path, card| {
            if path.trim().is_empty() || path.len() > 4096 {
                return false;
            }
            if card
                .status
                .as_deref()
                .is_some_and(|status| !known.contains(status))
            {
                card.status = None;
            }
            card.status.is_some() || card.pinned
        });
        while self.cards.len() > WORKSPACE_BOARD_CARD_COUNT_MAX {
            let Some(first) = self.cards.keys().next().cloned() else {
                break;
            };
            self.cards.remove(&first);
        }
        self
    }

    pub(super) fn knows_status(&self, id: &str) -> bool {
        self.statuses.iter().any(|status| status.id == id)
    }

    pub(super) fn patch_status(&mut self, patch: WorkspaceBoardStatusPatch) -> Result<(), String> {
        match patch {
            WorkspaceBoardStatusPatch::Rename { id, label } => {
                let Some(status) = self.statuses.iter_mut().find(|status| status.id == id) else {
                    return Err(format!("unknown workspace status: {id}"));
                };
                let label = workspace_board_label(&label, "");
                if label.is_empty() {
                    return Err("상태 이름을 입력하세요".to_string());
                }
                status.label = label;
            }
            WorkspaceBoardStatusPatch::Appearance { id, color, icon } => {
                if color.is_none() && icon.is_none() {
                    return Err("바꿀 상태 모양이 없습니다".to_string());
                }
                if color
                    .as_deref()
                    .is_some_and(|value| !WORKSPACE_BOARD_COLORS.contains(&value))
                {
                    return Err("이 창이 아는 상태 색이 아닙니다".to_string());
                }
                if icon
                    .as_deref()
                    .is_some_and(|value| !WORKSPACE_BOARD_ICONS.contains(&value))
                {
                    return Err("이 창이 아는 상태 아이콘이 아닙니다".to_string());
                }
                let Some(status) = self.statuses.iter_mut().find(|status| status.id == id) else {
                    return Err(format!("unknown workspace status: {id}"));
                };
                if let Some(color) = color {
                    status.color = color;
                }
                if let Some(icon) = icon {
                    status.icon = icon;
                }
            }
            WorkspaceBoardStatusPatch::Move { id, direction } => {
                if ![-1, 1].contains(&direction) {
                    return Err("상태는 한 칸씩만 옮길 수 있습니다".to_string());
                }
                let Some(index) = self.statuses.iter().position(|status| status.id == id) else {
                    return Err(format!("unknown workspace status: {id}"));
                };
                let next = index as isize + direction as isize;
                if next < 0 || next >= self.statuses.len() as isize {
                    return Ok(());
                }
                self.statuses.swap(index, next as usize);
            }
            WorkspaceBoardStatusPatch::Add => {
                if self.statuses.len() >= WORKSPACE_BOARD_STATUS_COUNT_MAX {
                    return Err(format!(
                        "상태는 최대 {WORKSPACE_BOARD_STATUS_COUNT_MAX}개까지 만들 수 있습니다"
                    ));
                }
                let label = format!("Status {}", self.statuses.len() + 1);
                let used: BTreeSet<_> = self
                    .statuses
                    .iter()
                    .map(|status| status.id.clone())
                    .collect();
                self.statuses.push(WorkspaceBoardStatus {
                    id: unused_workspace_board_status_id(
                        &workspace_board_status_id(&label, &label),
                        &used,
                    ),
                    label,
                    color: "neutral".to_string(),
                    icon: "circle-dot".to_string(),
                });
            }
            WorkspaceBoardStatusPatch::Remove { id } => {
                if self.statuses.len() <= 1 {
                    return Err("마지막 상태는 지울 수 없습니다".to_string());
                }
                let Some(index) = self.statuses.iter().position(|status| status.id == id) else {
                    return Err(format!("unknown workspace status: {id}"));
                };
                self.statuses.remove(index);
                let fallback = self.statuses[index.min(self.statuses.len() - 1)].id.clone();
                for card in self.cards.values_mut() {
                    if card.status.as_deref() == Some(id.as_str()) {
                        card.status = Some(fallback.clone());
                    }
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub(super) enum WorkspaceBoardStatusPatch {
    Rename {
        id: String,
        label: String,
    },
    Appearance {
        id: String,
        color: Option<String>,
        icon: Option<String>,
    },
    Move {
        id: String,
        direction: i8,
    },
    Add,
    Remove {
        id: String,
    },
}

/// How the sidebar gathers its rows.
///
/// Orca offers a fourth, `pr-status`, which is deliberately absent: it reads a
/// branch-to-pull-request cache a background poller fills, and we keep none —
/// asking GitHub once per branch inside the refresh that every workspace click
/// runs is not a grouping, it is a stall.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum SidebarGroupBy {
    #[default]
    Repo,
    #[serde(rename = "none")]
    Flat,
    State,
    /// Orca's fourth mode (`GROUP_BY_OPTIONS`: none/Status/PR/Project),
    /// bucketing by each checkout's review state through the same 60-second
    /// cache the board's pills read.
    Pr,
}

/// What orders the rows inside whatever gathers them.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum SidebarSortBy {
    /// The catalogue's own order — the nearest true thing to Orca's Manual
    /// while rows cannot be dragged.
    #[default]
    #[serde(rename = "default")]
    Catalog,
    Name,
    Recent,
    Activity,
    /// Orca's fifth choice (SORT_OPTIONS "Project"): the flat list gathered
    /// by each row's repository name without becoming a grouping.
    Repo,
}

/// What orders the project headers, when there are project headers.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum SidebarProjectOrder {
    #[default]
    #[serde(rename = "default")]
    Catalog,
    Recent,
}

/// The sidebar's three ways of looking at one list.
///
/// One value and not three, because they are one fact with three faces and
/// they are chosen from one menu — three settings would let a window arrive
/// having absorbed two thirds of a choice.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SidebarView {
    #[serde(default)]
    pub(super) group_by: SidebarGroupBy,
    #[serde(default)]
    pub(super) sort_by: SidebarSortBy,
    #[serde(default)]
    pub(super) project_order: SidebarProjectOrder,
}

impl SidebarView {
    /// The three words the window sent, or a refusal naming the one that was
    /// not a word this build knows.
    ///
    /// Refused rather than quietly folded to the default: a silent fold makes
    /// what the person chose and what the window draws disagree, and the
    /// difference only ever surfaces after a restart.
    pub(super) fn parsed(
        group_by: &str,
        sort_by: &str,
        project_order: &str,
    ) -> Result<Self, String> {
        let group = match group_by {
            "repo" => SidebarGroupBy::Repo,
            "none" => SidebarGroupBy::Flat,
            "state" => SidebarGroupBy::State,
            "pr" => SidebarGroupBy::Pr,
            other => return Err(format!("unknown sidebar grouping: {other}")),
        };
        let sort = match sort_by {
            "default" => SidebarSortBy::Catalog,
            "name" => SidebarSortBy::Name,
            "recent" => SidebarSortBy::Recent,
            "activity" => SidebarSortBy::Activity,
            "repo" => SidebarSortBy::Repo,
            other => return Err(format!("unknown sidebar sort: {other}")),
        };
        let order = match project_order {
            "default" => SidebarProjectOrder::Catalog,
            "recent" => SidebarProjectOrder::Recent,
            other => return Err(format!("unknown sidebar project order: {other}")),
        };
        Ok(Self {
            group_by: group,
            sort_by: sort,
            project_order: order,
        })
    }
}

/// 카드가 에이전트(워커) 행을 요약하는가, 전부 세우는가 — Orca의
/// `agentActivityDisplayMode`(AGENT_ACTIVITY_DISPLAY_OPTIONS: Compact /
/// Full list)이고 기본도 원본과 같은 compact다
/// (DEFAULT_AGENT_ACTIVITY_DISPLAY_MODE, store 실측).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum AgentActivityDisplay {
    #[default]
    Compact,
    Full,
}

/// The order of ordinary source-control groups. Conflicts are deliberately
/// absent because Orca pins them ahead of every selectable preset.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum SourceControlGroupOrder {
    #[default]
    #[serde(rename = "changes-first")]
    Changes,
    #[serde(rename = "staged-first")]
    Staged,
    #[serde(rename = "untracked-first")]
    Untracked,
}

/// Which implicit base the committed-changes view follows when neither the
/// current worktree nor its repository has pinned one. These are Orca's two
/// displayed values; an enum keeps the renderer from turning a boolean into a
/// second, drifting vocabulary.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum SourceControlCompareBase {
    #[default]
    RepositoryDefault,
    BranchUpstream,
}

/// Where the setup command is shown while a newly-created workspace is
/// prepared. These are Orca's wire values; keeping the axis in the type means
/// the renderer never invents a fourth spelling or silently falls back.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum SetupScriptLaunchMode {
    #[default]
    NewTab,
    SplitVertical,
    SplitHorizontal,
}

/// Who owns a shortcut that could be interpreted by both the application and
/// the focused terminal. The wire values and default match Orca exactly.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum TerminalShortcutPolicy {
    #[default]
    OrcaFirst,
    TerminalFirst,
}

/// Where newly-created workspaces live.
///
/// Orca treats a relative directory as repository-relative and, by default,
/// adds one repository-named level before the workspace slug. Keeping those
/// two answers together prevents the path resolver, Settings and automation
/// runs from inventing three subtly different layouts.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct WorkspaceCreationPrefs {
    #[serde(default = "default_workspace_directory")]
    pub(super) directory: String,
    #[serde(default = "enabled_by_default")]
    pub(super) nest_workspaces: bool,
    /// Every layout workspaces were made under before this one.
    ///
    /// Kept because changing either field moves where new workspaces go
    /// WITHOUT moving the ones already made: without this list they stop
    /// matching the only path fingerprint ownership has, and a person whose
    /// branch prefix is `none` watches their sidebar empty. Orca carries the
    /// same list for the same reason (`workspaceDirHistory`,
    /// `ownership.ts:33-74`).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) history: Vec<WorkspaceLayout>,
}

/// One remembered answer to "where do workspaces go".
///
/// The pair travels together because neither half decides a path alone: the
/// same directory with nesting on and off puts a workspace two different
/// places (`buildWorkspaceDirHistoryForUpdate`,
/// `terminal-settings-migrations.ts:29-32`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct WorkspaceLayout {
    pub(super) directory: String,
    pub(super) nest_workspaces: bool,
}

impl Default for WorkspaceCreationPrefs {
    fn default() -> Self {
        Self {
            // `~` is resolved only at the filesystem boundary. The portable
            // stored value keeps fresh-install fixtures and copied settings
            // independent of whichever account ran the build.
            directory: default_workspace_directory(),
            nest_workspaces: true,
            history: Vec::new(),
        }
    }
}

impl WorkspaceCreationPrefs {
    pub(super) fn normalized(mut self) -> Self {
        self.directory = self.directory.trim().to_string();
        if self.directory.is_empty() {
            self.directory = default_workspace_directory();
        }
        self
    }
}

pub(super) fn default_workspace_directory() -> String {
    "~/zerocode/workspaces".to_string()
}

#[derive(Clone, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(super) enum WorkspaceCreationPrefsPatch {
    Directory(String),
    NestWorkspaces(bool),
}

/// The most layouts this list will hold.
///
/// Bounded because it is user data that grows every time somebody changes
/// their mind, and an unbounded list is a defect by this tree's own rule. The
/// oldest is dropped first: a layout somebody moved away from five changes ago
/// is the least likely to still have workspaces under it, and dropping it
/// costs at most one row's visibility — never the row itself, since
/// `unknown-legacy` still shows.
pub(super) const WORKSPACE_LAYOUT_HISTORY_MAX: usize = 16;

impl WorkspaceCreationPrefsPatch {
    pub(super) fn apply(self, prefs: &mut WorkspaceCreationPrefs) -> Result<(), String> {
        let was = WorkspaceLayout {
            directory: prefs.directory.clone(),
            nest_workspaces: prefs.nest_workspaces,
        };
        match self {
            Self::Directory(directory) => {
                let directory = directory.trim();
                if directory.is_empty() {
                    return Err("워크스페이스 디렉터리를 입력하세요".to_string());
                }
                prefs.directory = directory.to_string();
            }
            Self::NestWorkspaces(value) => prefs.nest_workspaces = value,
        }
        prefs.remember(was);
        Ok(())
    }
}

impl WorkspaceCreationPrefs {
    /// Write down where workspaces used to go, if this change moved them.
    ///
    /// Nothing is remembered when the answer did not actually change — Orca
    /// makes the same comparison before appending, so that saving the same
    /// value twice does not grow the list
    /// (`buildWorkspaceDirHistoryForUpdate`, `terminal-settings-migrations.ts:22-27`).
    pub(super) fn remember(&mut self, was: WorkspaceLayout) {
        if was.directory == self.directory && was.nest_workspaces == self.nest_workspaces {
            return;
        }
        if was.directory.trim().is_empty() {
            return;
        }
        // The layout now in force is not history, and neither is one already
        // written down.
        self.history.retain(|held| held != &was);
        self.history.push(was);
        let now = WorkspaceLayout {
            directory: self.directory.clone(),
            nest_workspaces: self.nest_workspaces,
        };
        self.history.retain(|held| held != &now);
        while self.history.len() > WORKSPACE_LAYOUT_HISTORY_MAX {
            self.history.remove(0);
        }
    }
}

/// Which door summons the floating workspace — Orca's
/// `FloatingTerminalTriggerLocation` (`ui-chrome-types`), same wire words.
/// Exactly one of the two shows at a time; the chord works under either.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum FloatingTriggerLocation {
    #[default]
    FloatingButton,
    StatusBar,
}

/// The floating workspace's three answers — Orca's `floatingTerminalEnabled`
/// / `floatingTerminalCwd` / `floatingTerminalTriggerLocation`, defaults as
/// measured (`constants.ts:289-294`: on, `~`, floating-button).
///
/// Orca's fourth field, `floatingTerminalTrustedCwds`, is deliberately not
/// carried: it is an Electron-sandbox grant ledger for creating markdown
/// notes in picker-approved directories, and this panel hosts one plain
/// shell — the preference seats a process the person opens, it widens no
/// filesystem scope, so there is nothing for a trust list to guard.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct FloatingWorkspacePrefs {
    #[serde(default = "enabled_by_default")]
    pub(super) enabled: bool,
    #[serde(default = "default_floating_workspace_cwd")]
    pub(super) cwd: String,
    #[serde(default)]
    pub(super) trigger_location: FloatingTriggerLocation,
}

impl Default for FloatingWorkspacePrefs {
    fn default() -> Self {
        Self {
            enabled: true,
            cwd: default_floating_workspace_cwd(),
            trigger_location: FloatingTriggerLocation::default(),
        }
    }
}

impl FloatingWorkspacePrefs {
    /// Purely lexical, like every other `normalized()` arm: a stored seat
    /// that stopped existing is a use-time question ([`
    /// resolved_floating_workspace_seat`] falls back), not a load-time edit —
    /// rewriting it here would erase the person's choice the first time a
    /// network volume was late to mount.
    pub(super) fn normalized(mut self) -> Self {
        let cwd = self.cwd.trim();
        self.cwd = if cwd.is_empty() {
            default_floating_workspace_cwd()
        } else {
            cwd.to_string()
        };
        self
    }
}

pub(super) fn default_floating_workspace_cwd() -> String {
    "~".to_string()
}

#[derive(Clone, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(super) enum FloatingWorkspacePatch {
    Enabled(bool),
    Cwd(String),
    TriggerLocation(FloatingTriggerLocation),
}

impl FloatingWorkspacePatch {
    pub(super) fn apply(self, prefs: &mut FloatingWorkspacePrefs) -> Result<(), String> {
        match self {
            Self::Enabled(value) => prefs.enabled = value,
            Self::Cwd(path) => {
                let asked = path.trim();
                if asked.is_empty() || asked == "~" {
                    prefs.cwd = default_floating_workspace_cwd();
                    return Ok(());
                }
                // The pane's only writer is the OS folder panel, which
                // answers in absolute paths that exist — but a write is
                // still a write, and a seat that is not a directory would
                // fail every shell this panel opens. Refused loudly here;
                // only a directory that disappears LATER falls back quietly.
                let seat = expanded_floating_workspace_cwd(asked)?;
                if !seat.is_dir() {
                    return Err(format!("{asked}는 디렉터리가 아닙니다"));
                }
                prefs.cwd = seat.to_string_lossy().into_owned();
            }
            Self::TriggerLocation(location) => prefs.trigger_location = location,
        }
        Ok(())
    }
}

/// `~` / `~/x` / a relative path → the absolute path those words mean —
/// Orca's `resolveFloatingWorkspaceInput` (floating-workspace-directory.ts):
/// bare `~` is home, a `~`-prefixed path expands under it, and a relative
/// path resolves against home rather than against wherever this process
/// happens to sit. Purely lexical: existence is the caller's question, and
/// the two callers ask it differently — a write refuses a missing directory,
/// a spawn falls back to home.
pub(super) fn expanded_floating_workspace_cwd(asked: &str) -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("홈 디렉터리가 없습니다")?;
    let asked = asked.trim();
    if asked.is_empty() || asked == "~" {
        return Ok(home);
    }
    let expanded = asked
        .strip_prefix("~/")
        .or_else(|| asked.strip_prefix("~\\"))
        .map_or_else(|| PathBuf::from(asked), |tail| home.join(tail));
    Ok(if expanded.is_absolute() {
        expanded
    } else {
        home.join(expanded)
    })
}

/// Where the floating shell starts right now, from the stored preference.
///
/// Orca's `resolveFloatingTerminalCwd` falls back when the stored directory
/// stopped being one; this window falls back to HOME where Orca chooses its
/// app-owned notes directory, because there are no notes here to own — a
/// shell can always start at home, and an invisible app directory would be
/// a seat nobody chose and nobody can see.
pub(super) fn resolved_floating_workspace_seat(
    prefs: &FloatingWorkspacePrefs,
) -> Result<PathBuf, String> {
    let seat = expanded_floating_workspace_cwd(&prefs.cwd)?;
    if seat.is_dir() {
        Ok(seat)
    } else {
        dirs::home_dir().ok_or("홈 디렉터리가 없습니다".to_string())
    }
}

/// The seat as the floating panel's header says it: the home prefix folds to
/// `~`, anything else is the path as it stands. Presentation only — the
/// stored preference and the spawned cwd stay absolute.
pub(super) fn floating_seat_label(seat: &Path) -> String {
    let Some(home) = dirs::home_dir() else {
        return seat.display().to_string();
    };
    if seat == home {
        return "~".to_string();
    }
    match seat.strip_prefix(&home) {
        Ok(tail) => format!("~{}{}", std::path::MAIN_SEPARATOR, tail.display()),
        Err(_) => seat.display().to_string(),
    }
}

pub(super) const OPEN_IN_APPLICATIONS_MAX: usize = 8;

/// One external editor offered from a workspace's Open in menu.
///
/// `command` is parsed into argv by this process and never handed to a shell.
/// The workspace path is appended as its own argument at launch time, so a
/// repository name cannot become command text.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct OpenInApplication {
    pub(super) id: String,
    pub(super) label: String,
    pub(super) command: String,
}

/// The name Launch Services knows a preset by, where the label this window
/// prints is not it. Measured on a machine with the editors on it: `Zed.app`
/// and `Cursor.app` are named exactly as their labels, and VS Code is not —
/// asking for "VS Code" resolves nothing at all, while the bundle itself is
/// "Visual Studio Code".
pub(super) const MACOS_APPLICATION_NAMES: &[(&str, &str)] = &[("vscode", "Visual Studio Code")];

/// What to ask macOS for: the preset's real application name when this
/// window's label is not it, and the label otherwise — which is also the only
/// thing a custom entry can be asked by, since nobody else named it.
pub(super) fn macos_application_name(application: &OpenInApplication) -> &str {
    MACOS_APPLICATION_NAMES
        .iter()
        .find(|(id, _)| *id == application.id)
        .map_or(application.label.as_str(), |(_, name)| *name)
}

pub(super) fn open_in_application_presets() -> Vec<OpenInApplication> {
    [
        ("vscode", "VS Code", "code"),
        ("cursor", "Cursor", "cursor"),
        ("zed", "Zed", "zed"),
    ]
    .into_iter()
    .map(|(id, label, command)| OpenInApplication {
        id: id.to_string(),
        label: label.to_string(),
        command: command.to_string(),
    })
    .collect()
}

pub(super) fn default_open_in_applications() -> Vec<OpenInApplication> {
    open_in_application_presets().into_iter().take(1).collect()
}

pub(super) fn normalized_open_in_application(
    mut application: OpenInApplication,
    fallback_id: Option<String>,
) -> Option<OpenInApplication> {
    application.id = application.id.trim().to_string();
    application.label = application.label.trim().to_string();
    application.command = application.command.trim().to_string();
    if application.id.is_empty() {
        application.id = fallback_id?;
    }
    if application.id.len() > 128
        || application.id.chars().any(char::is_control)
        || application.label.is_empty()
        || application.command.is_empty()
    {
        return None;
    }
    Some(application)
}

pub(super) fn normalize_open_in_applications(
    applications: Vec<OpenInApplication>,
) -> Vec<OpenInApplication> {
    let mut seen = BTreeSet::new();
    applications
        .into_iter()
        .enumerate()
        .filter_map(|(index, application)| {
            normalized_open_in_application(application, Some(format!("application-{}", index + 1)))
        })
        .filter(|application| seen.insert(application.id.clone()))
        .take(OPEN_IN_APPLICATIONS_MAX)
        .collect()
}

#[derive(Clone, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(super) enum OpenInApplicationsPatch {
    Upsert(OpenInApplication),
    Remove(String),
}

impl OpenInApplicationsPatch {
    pub(super) fn apply(self, applications: &mut Vec<OpenInApplication>) -> Result<(), String> {
        match self {
            Self::Upsert(application) => {
                let application = normalized_open_in_application(application, None)
                    .ok_or_else(|| "앱 이름과 명령을 입력하세요".to_string())?;
                if let Some(existing) = applications
                    .iter_mut()
                    .find(|existing| existing.id == application.id)
                {
                    *existing = application;
                } else {
                    if applications.len() >= OPEN_IN_APPLICATIONS_MAX {
                        return Err(format!(
                            "Open in 앱은 최대 {OPEN_IN_APPLICATIONS_MAX}개까지 추가할 수 있습니다"
                        ));
                    }
                    applications.push(application);
                }
            }
            Self::Remove(id) => {
                let id = id.trim();
                applications.retain(|application| application.id != id);
            }
        }
        Ok(())
    }
}

#[derive(Clone, Serialize)]
pub(super) struct OpenInApplicationsSpec {
    pub(super) max: usize,
    pub(super) presets: Vec<OpenInApplication>,
}

impl Default for OpenInApplicationsSpec {
    fn default() -> Self {
        Self {
            max: OPEN_IN_APPLICATIONS_MAX,
            presets: open_in_application_presets(),
        }
    }
}

/// One remembered browser tab: the address and the dress it wore — the
/// mobile emulator's viewport box and its reader (1-g25, live report
/// "잔버그": a restored emulator came back as a desktop tab). Files written
/// before this carried bare address strings; `browser_tabs_stored` still
/// reads them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct StoredBrowserTab {
    pub(super) url: String,
    #[serde(default)]
    pub(super) viewport: Option<String>,
    /// The old spelling of the reader — `true` was the iPhone (1-g25). Still
    /// read, and written back only while true, so a file the window has not
    /// rewritten keeps its fact; the window reads it as the iPhone reader.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub(super) mobile: bool,
    /// The reader the pane wore, as a string (t-3043 §2.5): the emulator's
    /// iPhone, or the agent an `open` asked for. Absent for a desktop pane —
    /// including one the per-site table dressed, which the table dresses
    /// again at the next boot rather than the record freezing the name.
    #[serde(default)]
    pub(super) reader: Option<String>,
    /// The profile whose cookie jar the pane stood in (1-g26) — an id from
    /// `create_browser_profile`, or absent for the shared default store.
    #[serde(default)]
    pub(super) profile: Option<String>,
}

pub(super) fn browser_tabs_stored<'de, D>(
    deserializer: D,
) -> Result<Vec<StoredBrowserTab>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Read {
        Bare(String),
        Dressed(StoredBrowserTab),
    }
    Ok(Vec::<Read>::deserialize(deserializer)?
        .into_iter()
        .map(|one| match one {
            Read::Bare(url) => StoredBrowserTab {
                url,
                viewport: None,
                mobile: false,
                reader: None,
                profile: None,
            },
            Read::Dressed(tab) => tab,
        })
        .collect())
}

/// One remembered address-bar visit — what the suggestion strip ranks. The
/// count and the last-visit instant are the two facts Orca's score reads
/// (prefix +100, visits capped at 50, recency fading over 24h); the title is
/// the row's words. Page CONTENT never belongs here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct StoredBrowserVisit {
    pub(super) url: String,
    #[serde(default)]
    pub(super) title: String,
    #[serde(default)]
    pub(super) count: u32,
    /// Epoch milliseconds of the last visit. The renderer's clock wrote it
    /// and reads it back; nothing on this side does arithmetic with it.
    #[serde(default)]
    pub(super) at: f64,
}

/// How many visits the ledger keeps. Our own resource bound — Orca does not
/// surface its cap — sized so a season of browsing ranks instantly and the
/// settings file stays a file, not a database.
pub(super) const BROWSER_VISITS_KEPT: usize = 200;

/// One row of the per-site user-agent table (browser-door-for-agents §2.5):
/// a pane born on a URL whose host is exactly `host` wears `agent` instead
/// of this machine's Safari name. Exact host in v1 — no parent-domain rows —
/// so `confluence.example.com` and `example.com` are two rows and a row never
/// surprises a sibling site. A running pane keeps the agent it was born with:
/// a user agent is builder-time in wry, so a row means 「다음 판부터」.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct SiteUserAgent {
    pub(super) host: String,
    pub(super) agent: String,
}

/// The longest agent a row keeps. Real browsers say themselves in under 200
/// characters; the bound stops a pasted page from becoming a row.
pub(super) const BROWSER_USER_AGENT_MAX_CHARS: usize = 512;

/// How many rows the table keeps — a per-site override is a handful of
/// sites, never a directory of the web.
pub(super) const BROWSER_USER_AGENTS_KEPT: usize = 100;

/// The host a row is keyed by: lowercase, no scheme, no port, no path, no
/// trailing dot — exactly what `Url::host_str()` answers for the page the
/// pane is born on, so the lookup is a plain comparison. An IDN host is kept
/// the way the URL will spell it (punycode). A whole URL, a port or a path is
/// refused rather than trimmed: a row that could never match is a lie in the
/// table, and the person who typed it would never learn why.
pub(super) fn canonical_site_host(raw: &str) -> Result<String, String> {
    let trimmed = raw.trim().trim_end_matches('.');
    let plain = !trimmed.is_empty()
        && !trimmed
            .chars()
            .any(|c| c.is_whitespace() || matches!(c, '/' | ':' | '@' | '?' | '#'));
    plain
        .then(|| url::Url::parse(&format!("https://{trimmed}/")).ok())
        .flatten()
        .and_then(|url| url.host_str().map(str::to_string))
        .ok_or_else(|| {
            format!("호스트는 example.com 처럼 스킴·포트·경로 없는 이름이어야 합니다: {raw}")
        })
}

/// A row as the table keeps it: the canonical host and a trimmed agent that
/// is one printable ASCII line within the table's length — what an HTTP
/// header can carry and WebKit will send unchanged.
pub(super) fn canonical_site_user_agent(row: SiteUserAgent) -> Result<SiteUserAgent, String> {
    let host = canonical_site_host(&row.host)?;
    let agent = row.agent.trim();
    if agent.is_empty() || !agent.chars().all(|c| (' '..='~').contains(&c)) {
        return Err("사용자 에이전트는 출력 가능한 ASCII 한 줄이어야 합니다".to_string());
    }
    if agent.chars().count() > BROWSER_USER_AGENT_MAX_CHARS {
        return Err(format!(
            "사용자 에이전트는 {BROWSER_USER_AGENT_MAX_CHARS}자 이하여야 합니다"
        ));
    }
    Ok(SiteUserAgent {
        host,
        agent: agent.to_string(),
    })
}

// Orca uses the same logarithmic half-step ladder for its application UI and
// browser pages. The persisted fields and consumers remain separate; only
// the numeric contract has one owner here.
pub(super) const ZOOM_MIN_LEVEL: f64 = -3.0;
pub(super) const ZOOM_MAX_LEVEL: f64 = 5.0;
pub(super) const ZOOM_STEP: f64 = 0.5;
pub(super) const ZOOM_DEFAULT_LEVEL: f64 = 0.0;
pub(super) const ZOOM_SCALE_BASE: f64 = 1.2;

pub(super) fn normalize_zoom_level(level: f64) -> f64 {
    if !level.is_finite() {
        return ZOOM_DEFAULT_LEVEL;
    }
    let normalized = (level / ZOOM_STEP).round() * ZOOM_STEP;
    let clamped = normalized.clamp(ZOOM_MIN_LEVEL, ZOOM_MAX_LEVEL);
    if clamped == 0.0 { 0.0 } else { clamped }
}

pub(super) fn zoom_level_to_scale(level: f64) -> f64 {
    ZOOM_SCALE_BASE.powf(normalize_zoom_level(level))
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(super) struct ZoomLevelSpec {
    pub(super) min_level: f64,
    pub(super) max_level: f64,
    pub(super) step: f64,
    pub(super) default_level: f64,
    pub(super) scale_base: f64,
}

impl Default for ZoomLevelSpec {
    fn default() -> Self {
        Self {
            min_level: ZOOM_MIN_LEVEL,
            max_level: ZOOM_MAX_LEVEL,
            step: ZOOM_STEP,
            default_level: ZOOM_DEFAULT_LEVEL,
            scale_base: ZOOM_SCALE_BASE,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct BrowserPrefs {
    #[serde(default)]
    pub(super) home_page: String,
    #[serde(default)]
    pub(super) search_engine: BrowserSearchEngine,
    #[serde(default)]
    pub(super) restore_tabs: bool,
    /// Route http(s) links from app-owned content into the workspace browser.
    /// `false` is Orca's default: leave the workspace intact and hand the URL
    /// to the operating system browser.
    #[serde(default)]
    pub(super) open_links_in_app: bool,
    /// When enabled, Shift plus the platform primary modifier chooses the
    /// opposite destination. With the default `false`, that chord always
    /// means the system browser, matching Orca's link-routing contract.
    #[serde(default)]
    pub(super) open_links_in_app_modifier_inverts: bool,
    /// Whether the terminal's one-time link-routing question has been asked.
    /// The measured flow (`requestOpenLinksInAppPreference`) asks on the FIRST
    /// terminal link activation, stores the answer as `open_links_in_app`,
    /// and never asks again — dismissing counts as "system browser".
    #[serde(default)]
    pub(super) open_links_in_app_prompted: bool,
    /// The bare-click action popover on terminal links. Orca ships this ON
    /// (`terminalLinkActionPopoverEnabled`, shared/constants.ts:266); turned
    /// off, `getLinkActionContext` returns null so a bare click does nothing
    /// and only the modifier chord opens a terminal link.
    #[serde(default = "enabled_by_default")]
    pub(super) terminal_link_action_popover: bool,
    /// Orca's logarithmic browser zoom level. The renderer derives the page
    /// factor as `1.2 ^ level`; keeping the level here preserves its exact
    /// half-step contract instead of persisting rounded percentages.
    #[serde(default)]
    pub(super) default_zoom_level: f64,
    /// The browser tabs that were open when the app last described its
    /// session. Credentials and page content never belong here.
    #[serde(default, deserialize_with = "browser_tabs_stored")]
    pub(super) open_tabs: Vec<StoredBrowserTab>,
    /// The address bar's visit ledger, most recent first (1-g83's queued
    /// half). Bounded by [`BROWSER_VISITS_KEPT`] on every write.
    #[serde(default)]
    pub(super) visits: Vec<StoredBrowserVisit>,
    /// The per-site user-agent table (§2.5): the agent a pane born on that
    /// exact host wears when it was opened with no reader of its own. Rows
    /// are written one at a time through [`BrowserUserAgentPatch`], bounded
    /// by [`BROWSER_USER_AGENTS_KEPT`].
    #[serde(default)]
    pub(super) user_agents: Vec<SiteUserAgent>,
}

impl Default for BrowserPrefs {
    fn default() -> Self {
        Self {
            home_page: String::new(),
            search_engine: BrowserSearchEngine::default(),
            restore_tabs: false,
            open_links_in_app: false,
            open_links_in_app_modifier_inverts: false,
            open_links_in_app_prompted: false,
            // The one switch Orca ships ON (terminalLinkActionPopoverEnabled).
            terminal_link_action_popover: true,
            default_zoom_level: 0.0,
            open_tabs: Vec::new(),
            visits: Vec::new(),
            user_agents: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
// Each variant's name IS the wire contract: it serializes to the snake_case
// Browser settings key it patches.
pub(super) enum BrowserLinkRoutingPatch {
    OpenLinksInApp(bool),
    OpenLinksInAppModifierInverts(bool),
    OpenLinksInAppPrompted(bool),
    TerminalLinkActionPopover(bool),
}

impl BrowserLinkRoutingPatch {
    pub(super) fn apply(self, prefs: &mut BrowserPrefs) {
        match self {
            Self::OpenLinksInApp(value) => prefs.open_links_in_app = value,
            Self::OpenLinksInAppModifierInverts(value) => {
                prefs.open_links_in_app_modifier_inverts = value;
            }
            Self::OpenLinksInAppPrompted(value) => prefs.open_links_in_app_prompted = value,
            Self::TerminalLinkActionPopover(value) => prefs.terminal_link_action_popover = value,
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
// One ROW at a time, for the reason link routing is one field at a time: a
// stale Settings window replacing the whole table would erase the row a
// sibling window just wrote.
pub(super) enum BrowserUserAgentPatch {
    /// Write the row for its host — a new row at the end, or the same
    /// host's row in place.
    Set(SiteUserAgent),
    /// Drop the row for this host; a host with no row is already what was
    /// asked.
    Remove(String),
}

impl BrowserUserAgentPatch {
    pub(super) fn apply(self, prefs: &mut BrowserPrefs) -> Result<(), String> {
        match self {
            Self::Set(row) => {
                let row = canonical_site_user_agent(row)?;
                if let Some(held) = prefs
                    .user_agents
                    .iter_mut()
                    .find(|held| held.host.eq_ignore_ascii_case(&row.host))
                {
                    held.agent = row.agent;
                } else if prefs.user_agents.len() >= BROWSER_USER_AGENTS_KEPT {
                    return Err(format!(
                        "사이트별 사용자 에이전트는 {BROWSER_USER_AGENTS_KEPT}행까지입니다"
                    ));
                } else {
                    prefs.user_agents.push(row);
                }
            }
            Self::Remove(host) => {
                let host = canonical_site_host(&host)?;
                prefs
                    .user_agents
                    .retain(|held| !held.host.eq_ignore_ascii_case(&host));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct NotificationPrefs {
    /// 마스터 스위치 — Orca `settings.notifications.enabled`
    /// (ipc/notifications.ts:121-124): 꺼지면 종류별이 무엇이든 벨은 없다.
    /// 이 자리가 없던 것이 맵 P0-14의 세 번째 병이었다.
    #[serde(default = "enabled_by_default")]
    pub(super) enabled: bool,
    #[serde(default = "enabled_by_default")]
    pub(super) agent_attention: bool,
    #[serde(default = "enabled_by_default")]
    pub(super) agent_completion: bool,
}

/// Preferences shared by the file editor and editable diff surface.
///
/// The diff layout itself predates this record and remains the existing
/// `diff_side_by_side` setting. Auto-save and wrapping live here because both
/// document surfaces consume them through the same field-patch road.
pub(super) const EDITOR_AUTO_SAVE_DELAY_DEFAULT_MS: u32 = 1_000;
pub(super) const EDITOR_AUTO_SAVE_DELAY_BOUNDS_MS: (u32, u32) = (250, 10_000);
pub(super) const EDITOR_AUTO_SAVE_DELAY_STEP_MS: u32 = 250;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct EditingPrefs {
    #[serde(default)]
    pub(super) editor_auto_save: bool,
    #[serde(default = "default_editor_auto_save_delay_ms")]
    pub(super) editor_auto_save_delay_ms: u32,
    /// Orca's file editor minimap is opt-in and does not appear in diff views.
    #[serde(default)]
    pub(super) editor_minimap_enabled: bool,
    #[serde(default = "enabled_by_default")]
    pub(super) editor_word_wrap: bool,
    #[serde(default)]
    pub(super) diff_word_wrap: bool,
    /// Browser spelling underlines and suggestions while a Markdown document
    /// is being edited. Code and data files remain explicitly unchecked.
    #[serde(default = "enabled_by_default")]
    pub(super) rich_markdown_spellcheck_enabled: bool,
    /// Local Markdown annotations and their agent handoff controls. The notes
    /// themselves remain stored even while the controls are hidden.
    #[serde(default = "enabled_by_default")]
    pub(super) markdown_review_tools_enabled: bool,
    /// Empty follows the terminal's monospace face, matching Orca's editor
    /// resolver. A non-empty value is a CSS font-family string selected or
    /// typed by the person; installed-font discovery is only a convenience.
    #[serde(default)]
    pub(super) editor_font_family: String,
    /// Orca opens its combined-diff file tree only when this preference is on.
    /// A manual show/hide gesture remains a view-local session preference; this
    /// field supplies the initial state for a new application session.
    #[serde(default)]
    pub(super) combined_diff_file_tree_visible_by_default: bool,
    /// `None` follows the platform default (on for Linux/macOS, off for
    /// Windows). An explicit toggle survives moving the same profile.
    #[serde(default)]
    pub(super) primary_selection_middle_click_paste: Option<bool>,
    /// ⌘± on an editor-like surface (file, diff, preview) steps this level;
    /// the pixel size the window renders is the terminal's base font plus
    /// this, fenced to EDITOR_FONT_PX_BOUNDS. Persisted as the level, not
    /// the pixels, exactly as Orca stores `editorFontZoomLevel` — the base
    /// font can change under it and the zoom keeps meaning "N steps out".
    #[serde(default)]
    pub(super) editor_font_zoom: i32,
}

impl Default for EditingPrefs {
    fn default() -> Self {
        Self {
            editor_auto_save: false,
            editor_auto_save_delay_ms: EDITOR_AUTO_SAVE_DELAY_DEFAULT_MS,
            editor_minimap_enabled: false,
            editor_word_wrap: true,
            diff_word_wrap: false,
            rich_markdown_spellcheck_enabled: true,
            markdown_review_tools_enabled: true,
            editor_font_family: String::new(),
            combined_diff_file_tree_visible_by_default: false,
            primary_selection_middle_click_paste: None,
            editor_font_zoom: 0,
        }
    }
}

/// The ladder ⌘± walks on editor-like surfaces — Orca's own rungs
/// (`editor-font-zoom.ts:1-3`): levels −6..18, one per step, 0 is "as set".
pub(super) const EDITOR_FONT_ZOOM_LEVELS: (i32, i32) = (-6, 18);
/// The fence AFTER the level lands on the base font (`editor-font-zoom.ts:25`)
/// — extreme sizes break the surface, whatever the base was. The terminal's
/// per-pane zoom walks between the same posts (`useTerminalFontZoom.ts:27-29`).
pub(super) const EDITOR_FONT_PX_BOUNDS: (f64, f64) = (8.0, 32.0);

impl EditingPrefs {
    pub(super) fn clamped(mut self) -> Self {
        self.editor_auto_save_delay_ms = self.editor_auto_save_delay_ms.clamp(
            EDITOR_AUTO_SAVE_DELAY_BOUNDS_MS.0,
            EDITOR_AUTO_SAVE_DELAY_BOUNDS_MS.1,
        );
        self.editor_font_family = self.editor_font_family.trim().to_string();
        self.editor_font_zoom = self
            .editor_font_zoom
            .clamp(EDITOR_FONT_ZOOM_LEVELS.0, EDITOR_FONT_ZOOM_LEVELS.1);
        self
    }
}

pub(super) const fn default_editor_auto_save_delay_ms() -> u32 {
    EDITOR_AUTO_SAVE_DELAY_DEFAULT_MS
}

/// What the window may ask of the editor zoom, spelled by this side — the
/// same manner as `ui_zoom_spec`: the ladder lives HERE once, and the window
/// reads it instead of growing a second copy of Orca's constants.
#[derive(Clone, Copy, Serialize)]
pub(super) struct EditorFontZoomSpec {
    pub(super) min_level: i32,
    pub(super) max_level: i32,
    pub(super) step: i32,
    pub(super) default_level: i32,
    pub(super) px_min: f64,
    pub(super) px_max: f64,
}

impl Default for EditorFontZoomSpec {
    fn default() -> Self {
        Self {
            min_level: EDITOR_FONT_ZOOM_LEVELS.0,
            max_level: EDITOR_FONT_ZOOM_LEVELS.1,
            step: 1,
            default_level: 0,
            px_min: EDITOR_FONT_PX_BOUNDS.0,
            px_max: EDITOR_FONT_PX_BOUNDS.1,
        }
    }
}

#[derive(Clone, Copy, Serialize)]
pub(super) struct EditingPrefsSpec {
    pub(super) auto_save_delay_ms: U32SettingSpec,
    pub(super) editor_font_zoom: EditorFontZoomSpec,
}

impl Default for EditingPrefsSpec {
    fn default() -> Self {
        Self {
            auto_save_delay_ms: U32SettingSpec {
                min: EDITOR_AUTO_SAVE_DELAY_BOUNDS_MS.0,
                max: EDITOR_AUTO_SAVE_DELAY_BOUNDS_MS.1,
                step: EDITOR_AUTO_SAVE_DELAY_STEP_MS,
            },
            editor_font_zoom: EditorFontZoomSpec::default(),
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(super) enum EditingPrefsPatch {
    EditorAutoSave(bool),
    EditorAutoSaveDelayMs(u32),
    EditorMinimapEnabled(bool),
    EditorWordWrap(bool),
    DiffWordWrap(bool),
    RichMarkdownSpellcheckEnabled(bool),
    MarkdownReviewToolsEnabled(bool),
    EditorFontFamily(String),
    CombinedDiffFileTreeVisibleByDefault(bool),
    PrimarySelectionMiddleClickPaste(bool),
    EditorFontZoom(i32),
}

impl EditingPrefsPatch {
    pub(super) fn apply(self, prefs: &mut EditingPrefs) {
        match self {
            Self::EditorAutoSave(value) => prefs.editor_auto_save = value,
            Self::EditorAutoSaveDelayMs(value) => prefs.editor_auto_save_delay_ms = value,
            Self::EditorMinimapEnabled(value) => prefs.editor_minimap_enabled = value,
            Self::EditorWordWrap(value) => prefs.editor_word_wrap = value,
            Self::DiffWordWrap(value) => prefs.diff_word_wrap = value,
            Self::RichMarkdownSpellcheckEnabled(value) => {
                prefs.rich_markdown_spellcheck_enabled = value;
            }
            Self::MarkdownReviewToolsEnabled(value) => {
                prefs.markdown_review_tools_enabled = value;
            }
            Self::EditorFontFamily(value) => prefs.editor_font_family = value,
            Self::CombinedDiffFileTreeVisibleByDefault(value) => {
                prefs.combined_diff_file_tree_visible_by_default = value;
            }
            Self::PrimarySelectionMiddleClickPaste(value) => {
                prefs.primary_selection_middle_click_paste = Some(value);
            }
            Self::EditorFontZoom(value) => prefs.editor_font_zoom = value,
        }
    }
}

impl SettingsDocument {
    /// The confirm policy as the operator reads it.
    pub(crate) const fn computer_confirm_policy(&self) -> crate::computer_use::confirm::Policy {
        crate::computer_use::confirm::Policy {
            payment: self.computer_confirm_payment,
            transfer: self.computer_confirm_transfer,
            delete: self.computer_confirm_delete,
        }
    }
}

/// Whether a live reflex run may start (`computer_live_reflex`), read from the
/// settings now — the reflex door asks it when a start comes, and nothing
/// else keeps a copy of it.
pub(crate) fn computer_live_reflex(repository: &settings::SettingsRepository) -> bool {
    load_settings_resilient(repository)
        .document
        .computer_live_reflex
}

pub(super) const fn enabled_by_default() -> bool {
    true
}

impl Default for NotificationPrefs {
    fn default() -> Self {
        Self {
            enabled: true,
            agent_attention: true,
            agent_completion: true,
        }
    }
}

/// Names written by releases that predate the canonical settings document.
///
/// Keeping every migration filename here prevents the adapter and its tests
/// from growing separate spellings of the same on-disk contract.
/// A read-only view of the pre-repository files in one explicit state root.
///
/// This type deliberately has no fallback to an implicit process-global state
/// directory. A test, portable install, or future profile that injects a
/// repository root must never import preferences from a different user's
/// global directory.
#[derive(Clone, Copy)]
pub(super) struct LegacySettings<'a> {
    pub(super) root: &'a Path,
}

impl<'a> LegacySettings<'a> {
    pub(super) fn new(root: &'a Path) -> Self {
        Self { root }
    }

    pub(super) fn read<T: serde::de::DeserializeOwned>(&self, file_name: &str) -> Option<T> {
        std::fs::read_to_string(self.root.join(file_name))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
    }

    pub(super) fn read_text(&self, file_name: &str) -> Option<String> {
        std::fs::read_to_string(self.root.join(file_name)).ok()
    }
}

pub(super) const DEFAULT_LEFT_SIDEBAR_TINT_COLOR: &str = "#18181b";
pub(super) const DEFAULT_LEFT_SIDEBAR_TINT_OPACITY: f64 = 0.08;
pub(super) const LEFT_SIDEBAR_TINT_OPACITY_BOUNDS: (f64, f64) = (0.0, 0.35);
pub(super) const LEFT_SIDEBAR_TINT_OPACITY_STEP: f64 = 0.01;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum LeftSidebarAppearanceMode {
    #[default]
    Default,
    MatchTerminal,
    Tinted,
}

pub(super) fn default_left_sidebar_tint_color() -> String {
    DEFAULT_LEFT_SIDEBAR_TINT_COLOR.to_string()
}

pub(super) const fn default_left_sidebar_tint_opacity() -> f64 {
    DEFAULT_LEFT_SIDEBAR_TINT_OPACITY
}

/// Keep the persisted colour in one CSS-safe spelling. Orca accepts three- or
/// six-digit hex (with or without `#`); an invalid old value falls back to its
/// measured default rather than escaping into a style declaration.
pub(super) fn normalized_optional_hex_color(color: &str) -> Option<String> {
    let digits = color.trim().strip_prefix('#').unwrap_or(color.trim());
    if !matches!(digits.len(), 3 | 6) || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    let expanded = if digits.len() == 3 {
        digits
            .chars()
            .flat_map(|glyph| [glyph, glyph])
            .collect::<String>()
    } else {
        digits.to_string()
    };
    Some(format!("#{}", expanded.to_ascii_lowercase()))
}

pub(super) fn normalized_hex_color(color: &str, fallback: &str) -> String {
    normalized_optional_hex_color(color).unwrap_or_else(|| fallback.to_string())
}

pub(super) fn normalized_left_sidebar_tint_color(color: &str) -> String {
    normalized_hex_color(color, DEFAULT_LEFT_SIDEBAR_TINT_COLOR)
}

pub(super) fn normalized_left_sidebar_tint_opacity(opacity: f64) -> f64 {
    if opacity.is_finite() {
        opacity.clamp(
            LEFT_SIDEBAR_TINT_OPACITY_BOUNDS.0,
            LEFT_SIDEBAR_TINT_OPACITY_BOUNDS.1,
        )
    } else {
        DEFAULT_LEFT_SIDEBAR_TINT_OPACITY
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(super) struct F64SettingSpec {
    pub(super) min: f64,
    pub(super) max: f64,
    pub(super) step: f64,
    pub(super) default: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub(super) struct LeftSidebarAppearanceSpec {
    pub(super) tint_color_default: &'static str,
    pub(super) tint_opacity: F64SettingSpec,
}

impl Default for LeftSidebarAppearanceSpec {
    fn default() -> Self {
        Self {
            tint_color_default: DEFAULT_LEFT_SIDEBAR_TINT_COLOR,
            tint_opacity: F64SettingSpec {
                min: LEFT_SIDEBAR_TINT_OPACITY_BOUNDS.0,
                max: LEFT_SIDEBAR_TINT_OPACITY_BOUNDS.1,
                step: LEFT_SIDEBAR_TINT_OPACITY_STEP,
                default: DEFAULT_LEFT_SIDEBAR_TINT_OPACITY,
            },
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(super) enum LeftSidebarAppearancePatch {
    Mode(LeftSidebarAppearanceMode),
    TintColor(String),
    TintOpacity(f64),
}

impl LeftSidebarAppearancePatch {
    pub(super) const fn setting_key(&self) -> &'static str {
        match self {
            Self::Mode(_) => setting_key::LEFT_SIDEBAR_APPEARANCE_MODE,
            Self::TintColor(_) => setting_key::LEFT_SIDEBAR_TINT_COLOR,
            Self::TintOpacity(_) => setting_key::LEFT_SIDEBAR_TINT_OPACITY,
        }
    }

    pub(super) fn apply(self, settings: &mut SettingsDocument) {
        match self {
            Self::Mode(mode) => settings.left_sidebar_appearance_mode = mode,
            Self::TintColor(color) => {
                settings.left_sidebar_tint_color = normalized_left_sidebar_tint_color(&color);
            }
            Self::TintOpacity(opacity) => {
                settings.left_sidebar_tint_opacity = normalized_left_sidebar_tint_opacity(opacity);
            }
        }
    }
}

/// `settings.update` (t-3191, docs/design/versioned-auto-update.md §2.3):
/// the policy, the channel, when the feed was last asked, and the one version
/// a person chose to skip. The words are the runtime's enums, so the row and
/// the judgement (`update_runtime`) cannot spell a policy two ways.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(super) struct UpdatePrefs {
    #[serde(default)]
    pub(super) policy: update_runtime::UpdatePolicy,
    #[serde(default)]
    pub(super) channel: update_runtime::UpdateChannel,
    /// UTC `YYYY-MM-DDTHH:MM:SSZ` of the last feed check that reached the
    /// feed (success or a feed-side word); written by `update_check`, never
    /// by the window. `None` until the first.
    #[serde(default)]
    pub(super) last_checked: Option<String>,
    /// 「이 버전 건너뛰기」: the announced version a person waved away. Cleared
    /// by the judgement when a newer version arrives (§2.3).
    #[serde(default)]
    pub(super) skipped_version: Option<String>,
}

impl Default for UpdatePrefs {
    fn default() -> Self {
        Self {
            policy: update_runtime::UpdatePolicy::Ask,
            channel: update_runtime::UpdateChannel::Stable,
            last_checked: None,
            skipped_version: None,
        }
    }
}

impl UpdatePrefs {
    /// A skipped version is a semver string or nothing; whitespace and an
    /// empty field are nothing. The clock word is kept as written — the
    /// judgement parses it and treats an unparseable one as 「never」.
    pub(super) fn normalized(mut self) -> Self {
        self.skipped_version = self
            .skipped_version
            .map(|held| held.trim().to_string())
            .filter(|held| semver::Version::parse(held).is_ok());
        self.last_checked = self
            .last_checked
            .map(|held| held.trim().to_string())
            .filter(|held| !held.is_empty());
        self
    }
}

/// One field at a time, the way the window's other rows are patched. The
/// clock (`last_checked`) has no patch: only the check writes it.
#[derive(Clone, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(super) enum UpdatePrefsPatch {
    Policy(update_runtime::UpdatePolicy),
    Channel(update_runtime::UpdateChannel),
    SkippedVersion(Option<String>),
}

impl UpdatePrefsPatch {
    pub(super) fn apply(self, prefs: &mut UpdatePrefs) {
        match self {
            Self::Policy(value) => prefs.policy = value,
            Self::Channel(value) => prefs.channel = value,
            Self::SkippedVersion(value) => prefs.skipped_version = value,
        }
    }
}

/// The update table as the window reads it: the clock's two numbers, the
/// cache's TTL and the words the two pickers may offer. Spelled here once
/// from `update_runtime`'s constants; the window never grows its own copy.
#[derive(Clone, Serialize)]
pub(super) struct UpdateSpec {
    pub(super) check_after_boot_secs: u64,
    pub(super) check_every_secs: u64,
    pub(super) history_ttl_secs: u64,
    pub(super) policies: &'static [&'static str],
    pub(super) channels: &'static [&'static str],
}

impl Default for UpdateSpec {
    fn default() -> Self {
        Self {
            check_after_boot_secs: update_runtime::CHECK_AFTER_BOOT_SECS,
            check_every_secs: update_runtime::CHECK_EVERY_SECS,
            history_ttl_secs: update_runtime::HISTORY_TTL_SECS,
            policies: &["ask", "auto", "off"],
            channels: &["stable", "beta"],
        }
    }
}

/// The versioned preference payload. Persistence metadata (format/revision)
/// belongs to `SettingsRepository`; this type owns only product semantics.
#[derive(Clone, PartialEq, Serialize, Deserialize)]
pub(super) struct SettingsDocument {
    #[serde(default)]
    pub(super) explorer: serde_json::Value,
    #[serde(default)]
    pub(super) skills: serde_json::Value,
    /// The PR depth's numbers overlay (`zerocode_core::checks::Limits`),
    /// keyed by field — `{"poll_ms": 6000}`. Empty is the table itself.
    #[serde(default)]
    pub(super) checks: serde_json::Value,
    /// The readiness snapshot's numbers overlay
    /// (`zerocode_core::readiness::Limits`), keyed by field —
    /// `{"launch_fresh_ms": 10000}`. Empty is the table itself.
    #[serde(default)]
    pub(super) readiness: serde_json::Value,
    #[serde(default)]
    pub(super) crash: serde_json::Value,
    #[serde(default = "default_system_choice")]
    pub(super) locale: String,
    #[serde(default = "default_system_choice")]
    pub(super) theme: String,
    #[serde(default)]
    pub(super) ui_zoom_level: f64,
    #[serde(default = "default_app_font_family")]
    pub(super) app_font_family: String,
    #[serde(default = "default_status_bar_items")]
    pub(super) status_bar_items: Vec<String>,
    /// The catalog as it stood when this document was last normalised — see
    /// [`adopt_unseen_status_bar_items`] for why "on" alone cannot answer.
    #[serde(default)]
    pub(super) status_bar_items_seen: Vec<String>,
    #[serde(default)]
    pub(super) usage_percentage_display: UsagePercentageDisplay,
    #[serde(default)]
    pub(super) status_bar_usage_mode: StatusBarUsageMode,
    /// Providers whose token ledger this window does NOT read.
    ///
    /// Stored as the OFF list rather than the on list, because reading is the
    /// default and an on-list would have to name every provider that exists —
    /// a document written before a provider was added would then read as
    /// having turned it off.
    #[serde(default)]
    pub(super) usage_analytics_off: Vec<String>,
    #[serde(default)]
    pub(super) source_control_view_mode: SourceControlViewMode,
    #[serde(default = "enabled_by_default")]
    pub(super) show_titlebar_app_name: bool,
    #[serde(default = "enabled_by_default")]
    pub(super) show_menu_bar_icon: bool,
    #[serde(default)]
    pub(super) minimize_to_tray_on_close: bool,
    #[serde(default)]
    pub(super) compact_worktree_cards: bool,
    /// 카드의 에이전트(워커) 행 차림. 다른 한 칸짜리 표시 설정처럼 저장된다
    /// — 재시작마다 요약으로 돌아오는 차림은 동작하지 않는 스위치로 읽힌다.
    #[serde(default)]
    pub(super) agent_activity_display: AgentActivityDisplay,
    #[serde(default = "enabled_by_default")]
    pub(super) show_git_ignored_files: bool,
    #[serde(default)]
    pub(super) source_control_group_order: SourceControlGroupOrder,
    #[serde(default)]
    pub(super) source_control_compare_base: SourceControlCompareBase,
    #[serde(default)]
    pub(super) refresh_local_base_ref_on_worktree_create: bool,
    #[serde(default)]
    pub(super) left_sidebar_appearance_mode: LeftSidebarAppearanceMode,
    #[serde(default = "default_left_sidebar_tint_color")]
    pub(super) left_sidebar_tint_color: String,
    #[serde(default = "default_left_sidebar_tint_opacity")]
    pub(super) left_sidebar_tint_opacity: f64,
    #[serde(default)]
    pub(super) panel_widths: PanelWidths,
    #[serde(default)]
    pub(super) editing_prefs: EditingPrefs,
    #[serde(default)]
    pub(super) update: UpdatePrefs,
    #[serde(default)]
    pub(super) terminal_prefs: TerminalPrefs,
    #[serde(default)]
    pub(super) terminal_command: String,
    #[serde(default)]
    pub(super) setup_script_launch_mode: SetupScriptLaunchMode,
    #[serde(default)]
    pub(super) terminal_shortcut_policy: TerminalShortcutPolicy,
    #[serde(default)]
    pub(super) hidden_shortcuts: Vec<String>,
    #[serde(default)]
    pub(super) keybindings: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    pub(super) hidden_task_sources: Vec<String>,
    #[serde(default = "default_task_source")]
    pub(super) default_task_source: String,
    #[serde(default)]
    pub(super) hide_automation_workspaces: bool,
    /// The sidebar's other two hide filters — Orca's `hideDefaultBranchWorkspace`
    /// and `hideDetachedHeadWorkspaces` (SidebarWorkspaceFilterSection.tsx),
    /// stored the way the automation filter is and for the same reason: a hide
    /// that resets on restart reads as a switch that does not work.
    #[serde(default)]
    pub(super) hide_default_branch_workspaces: bool,
    #[serde(default)]
    pub(super) hide_detached_head_workspaces: bool,
    #[serde(default)]
    pub(super) hide_sleeping_workspaces: bool,
    /// 잠자는 것을 쓸어 낼 때에도 기본 브랜치 행은 남기는가. **기본이 켬**이다
    /// — Orca의 `alwaysShowDefaultBranchWorkspace`가 그렇고, 이유가 같다:
    /// 폴더 워크스페이스와 분리된 HEAD의 main은 종종 그 프로젝트의 유일한
    /// 행이고, 그것을 쓸면 프로젝트 하나가 사이드바에서 통째로 사라진다.
    #[serde(default = "enabled_by_default")]
    pub(super) keep_default_branch_awake: bool,
    #[serde(default)]
    pub(super) hide_agent_scratch_workspaces: bool,
    /// 워크스페이스 카드가 그리는 것들. **켜진 것을 적는다** — 이 창이 내일
    /// 그릴 줄 알게 될 속성은 이 메뉴를 한 번도 연 적 없는 사람에게 보이는
    /// 채로 와야 하고, 꺼진 것을 적으면 새 속성은 언제나 켜져 있게 되지만 그건
    /// 사람이 끈 것을 되살리는 목록이 아니라 그냥 다른 목록이다.
    #[serde(default = "default_worktree_card_properties")]
    pub(super) worktree_card_properties: Vec<String>,
    /// The user-arranged workspace board: its lane vocabulary, per-workspace
    /// placement/pin metadata, and the shared lane width.
    ///
    /// One nested value because all three describe one surface. Flattening
    /// them into unrelated settings permits a status deletion to land without
    /// the card reassignment that makes the deletion valid.
    #[serde(default)]
    pub(super) workspace_board: WorkspaceBoardSettings,
    /// 사이드바 목록의 세 시선. 하나의 사실이므로 하나의 쓰기다.
    #[serde(default)]
    pub(super) sidebar_view: SidebarView,
    #[serde(default = "enabled_by_default")]
    pub(super) confirm_close_pinned: bool,
    #[serde(default)]
    pub(super) skip_close_terminal_with_running_process_confirm: bool,
    #[serde(default)]
    pub(super) ctrl_tab_order_mode: CtrlTabOrderMode,
    #[serde(default)]
    pub(super) workspace_creation_prefs: WorkspaceCreationPrefs,
    #[serde(default)]
    pub(super) floating_workspace: FloatingWorkspacePrefs,
    #[serde(default = "default_open_in_applications")]
    pub(super) open_in_applications: Vec<OpenInApplication>,
    #[serde(default)]
    pub(super) skip_delete_worktree_confirm: bool,
    #[serde(default)]
    pub(super) skip_delete_automation_confirm: bool,
    /// How long the artifact store keeps a row and its copied file. `0` is
    /// "the ledger's own days" — the default, so a person who never opens
    /// settings keeps reports exactly as long as the runs they belong to.
    /// Bounds are `zerocode_core::artifact::Limits`'; this is the overlay.
    #[serde(default)]
    pub(super) artifacts_retention_days: u32,
    #[serde(default = "default_vault_session_limit", rename = "vault.sessionLimit")]
    pub(super) vault_session_limit: usize,
    #[serde(default = "enabled_by_default")]
    pub(super) diff_side_by_side: bool,
    /// Whether a conversation folds each turn's tool work behind one summary
    /// row (the extension's 「Focus view」, 2.1.221). OFF unless this person
    /// has asked for it: the transcript's default is the whole story, and
    /// the fold is the thing they reach for when a turn grows long.
    #[serde(default)]
    pub(super) conversation_focus_view: bool,
    #[serde(default)]
    pub(super) window_material: WindowMaterial,
    #[serde(default)]
    pub(super) default_agent: zerocode_core::DefaultAgentPreference,
    #[serde(default)]
    pub(super) agent_teams_mode: TeamsMode,
    /// Whether the window may switch the Claude account by itself when the
    /// selected one nears its limit or a worker stands at its wall (t-7538):
    /// `off`, `ask` (the default — a line and a button), `auto`.
    #[serde(default)]
    pub(super) claude_autoswitch_mode: zerocode_core::account_autoswitch::AutoSwitchMode,
    #[serde(default)]
    pub(super) worktree_prefs: WorktreePrefs,
    #[serde(default)]
    pub(super) notifications: NotificationPrefs,
    /// Whether the machine is held awake while agents run — Orca's
    /// `computerAwakeMode` (on/off/auto). Off is what a person who never
    /// chose gets, exactly as Orca's normalizer answers.
    #[serde(default)]
    pub(super) computer_awake_mode: awake::ComputerAwakeMode,
    /// The last step the operator holds for the person (docs/design/
    /// computer-use-full-operator.md §1.5): payment, transfer, delete. On
    /// for a person who never chose.
    #[serde(default = "enabled_by_default")]
    pub(super) computer_confirm_payment: bool,
    #[serde(default = "enabled_by_default")]
    pub(super) computer_confirm_transfer: bool,
    #[serde(default = "enabled_by_default")]
    pub(super) computer_confirm_delete: bool,
    /// Whether a live reflex run may start on this desktop (realtime v1,
    /// t-9205). Off for a person who never chose: a run holds the hand and
    /// presses at its own pace, so it is theirs to turn on.
    #[serde(default)]
    pub(super) computer_live_reflex: bool,
    #[serde(default)]
    pub(super) browser: BrowserPrefs,
    /// Whether the emulators this window started stay up when it exits
    /// (docs/design/emulator-first-second-20260921.md D3). On for a person who
    /// never chose: a device left booted is the whole of the next pane's first
    /// second, and the row on disk is what lets the next window adopt it.
    #[serde(default = "enabled_by_default", rename = "emulator.keepBooted")]
    pub(super) emulator_keep_booted: bool,
    /// Whether the window puts the last-used device up while it boots (D4).
    #[serde(default = "enabled_by_default", rename = "emulator.prebootLastUsed")]
    pub(super) emulator_preboot_last_used: bool,
    /// How long a kept-booted fleet with no pane on it waits before it is put
    /// away (D3). `0` is never — a phone's worth of RAM held for as long as
    /// the person wants it held.
    #[serde(
        default = "default_emulator_idle_shutdown_minutes",
        rename = "emulator.idleShutdownMinutes"
    )]
    pub(super) emulator_idle_shutdown_minutes: u32,
    /// Whether an OpenCode Go session cookie is on file — Orca's own
    /// `opencodeCookieConfigured` (`rate-limit-types.ts:134` has the same flag
    /// for the sibling provider). The cookie ITSELF is never here: it goes to
    /// the OS keychain, and this is only what the window needs to know to
    /// decide whether the gauge has anything to ask for.
    #[serde(default)]
    pub(super) opencode_cookie_configured: bool,
    /// The workspace to read, when the person names one. Empty means "find
    /// it" — the site's own server function lists them.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(super) opencode_workspace: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) ssh_hosts: Vec<ssh_hosts::SshHostEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) remote_workspaces: Vec<remote_workspaces::RemoteWorkspaceEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) sftp_bookmarks: Vec<sftp_runtime::Location>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) remote_servers: Vec<remote_servers::RemoteServerEntry>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) ssh_targets: Vec<ssh_store::SshTarget>,
    /// `~/.ssh/config` aliases somebody deleted. It rides here rather than in
    /// a file of its own for the same reason the targets do, and it is the
    /// only thing standing between a deleted card and the silent sync that
    /// runs every time the SSH pane opens.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub(super) deleted_ssh_config_aliases: Vec<String>,
    /// Markdown folder configured as the local second-brain vault.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(super) second_brain_vault: String,
    /// The skill's weekly review runs as a zo cron turn in the vault — opt-in.
    /// The record itself lives in the vault's zo cron registry
    /// (`<vault>/.zo/registries/crons.json`, written by `zo cron`); this is
    /// the switch the card shows and the status road reads the receipt for.
    #[serde(default)]
    pub(super) second_brain_weekly_review: bool,
    /// Saved knowledge scenes per vault path (one JSON line per vault path).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) second_brain_scenes: BTreeMap<String, String>,
    /// The knowledge graph's last exploration per vault path (t-4140): one
    /// JSON line — mode, centre, depth — in the scenes' shape and beside
    /// them. The window reads it when the graph opens and writes it on every
    /// change; the rule that reads it never looks at the graph's size.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub(super) second_brain_explore: BTreeMap<String, String>,
    /// Who this machine's Google login belongs to, read once at sign-in.
    ///
    /// The TOKENS are never here — they live in the file zo owns
    /// (`google_login`). This is the name a card shows, cached because the
    /// only place it can be read from is a network endpoint and the card
    /// repaints on every settings open.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub(super) google_account_email: String,
}

pub(super) fn default_system_choice() -> String {
    String::new()
}

pub(super) fn default_app_font_family() -> String {
    DEFAULT_APP_FONT_FAMILY.to_string()
}

pub(super) fn normalized_app_font_family(family: &str) -> String {
    let family = family.trim();
    if family.is_empty() {
        default_app_font_family()
    } else {
        family.to_string()
    }
}

pub(super) fn default_task_source() -> String {
    "jira".to_string()
}

impl Default for SettingsDocument {
    fn default() -> Self {
        Self {
            explorer: serde_json::json!({}),
            skills: serde_json::json!({}),
            checks: serde_json::json!({}),
            readiness: serde_json::json!({}),
            crash: serde_json::json!({}),
            locale: "system".to_string(),
            theme: "system".to_string(),
            ui_zoom_level: ZOOM_DEFAULT_LEVEL,
            app_font_family: default_app_font_family(),
            status_bar_items: default_status_bar_items(),
            status_bar_items_seen: default_status_bar_items(),
            usage_percentage_display: UsagePercentageDisplay::Used,
            status_bar_usage_mode: StatusBarUsageMode::Verbose,
            usage_analytics_off: Vec::new(),
            source_control_view_mode: SourceControlViewMode::List,
            show_titlebar_app_name: true,
            show_menu_bar_icon: true,
            minimize_to_tray_on_close: false,
            compact_worktree_cards: false,
            agent_activity_display: AgentActivityDisplay::Compact,
            show_git_ignored_files: true,
            source_control_group_order: SourceControlGroupOrder::Changes,
            source_control_compare_base: SourceControlCompareBase::RepositoryDefault,
            refresh_local_base_ref_on_worktree_create: false,
            left_sidebar_appearance_mode: LeftSidebarAppearanceMode::Default,
            left_sidebar_tint_color: default_left_sidebar_tint_color(),
            left_sidebar_tint_opacity: default_left_sidebar_tint_opacity(),
            panel_widths: PanelWidths::default(),
            editing_prefs: EditingPrefs::default(),
            update: UpdatePrefs::default(),
            terminal_prefs: TerminalPrefs::default(),
            terminal_command: String::new(),
            setup_script_launch_mode: SetupScriptLaunchMode::NewTab,
            terminal_shortcut_policy: TerminalShortcutPolicy::OrcaFirst,
            hidden_shortcuts: Vec::new(),
            keybindings: BTreeMap::new(),
            hidden_task_sources: Vec::new(),
            default_task_source: default_task_source(),
            hide_automation_workspaces: false,
            hide_default_branch_workspaces: false,
            hide_detached_head_workspaces: false,
            hide_sleeping_workspaces: false,
            keep_default_branch_awake: true,
            hide_agent_scratch_workspaces: false,
            worktree_card_properties: default_worktree_card_properties(),
            workspace_board: WorkspaceBoardSettings::default(),
            sidebar_view: SidebarView::default(),
            confirm_close_pinned: true,
            skip_close_terminal_with_running_process_confirm: false,
            ctrl_tab_order_mode: CtrlTabOrderMode::Mru,
            workspace_creation_prefs: WorkspaceCreationPrefs::default(),
            floating_workspace: FloatingWorkspacePrefs::default(),
            open_in_applications: default_open_in_applications(),
            skip_delete_worktree_confirm: false,
            skip_delete_automation_confirm: false,
            artifacts_retention_days: 0,
            vault_session_limit: default_vault_session_limit(),
            diff_side_by_side: true,
            conversation_focus_view: false,
            window_material: WindowMaterial::default(),
            default_agent: zerocode_core::DefaultAgentPreference::Auto,
            agent_teams_mode: TeamsMode::default(),
            claude_autoswitch_mode: zerocode_core::account_autoswitch::AutoSwitchMode::default(),
            worktree_prefs: WorktreePrefs::default(),
            notifications: NotificationPrefs::default(),
            computer_awake_mode: awake::ComputerAwakeMode::default(),
            computer_confirm_payment: true,
            computer_confirm_transfer: true,
            computer_confirm_delete: true,
            computer_live_reflex: false,
            browser: BrowserPrefs::default(),
            emulator_keep_booted: true,
            emulator_preboot_last_used: true,
            emulator_idle_shutdown_minutes: default_emulator_idle_shutdown_minutes(),
            opencode_cookie_configured: false,
            opencode_workspace: String::new(),
            ssh_hosts: Vec::new(),
            remote_workspaces: Vec::new(),
            sftp_bookmarks: Vec::new(),
            remote_servers: Vec::new(),
            ssh_targets: Vec::new(),
            deleted_ssh_config_aliases: Vec::new(),
            second_brain_vault: String::new(),
            second_brain_weekly_review: false,
            second_brain_scenes: BTreeMap::new(),
            second_brain_explore: BTreeMap::new(),
            google_account_email: String::new(),
        }
    }
}

impl SettingsDocument {
    /// Import the pre-repository files exactly once. Keeping this adapter at
    /// the boundary avoids a flag day and leaves old installs lossless.
    pub(super) fn from_legacy(root: &Path) -> Self {
        let legacy = LegacySettings::new(root);
        Self {
            locale: stored_locale(&legacy),
            theme: stored_theme(&legacy),
            panel_widths: stored_panel_widths(&legacy),
            terminal_prefs: stored_terminal_prefs(&legacy),
            terminal_command: quote_command(&stored_terminal_command(&legacy)),
            hidden_shortcuts: stored_hidden_shortcuts(&legacy),
            keybindings: stored_keybindings(&legacy),
            hidden_task_sources: stored_hidden_task_sources(&legacy),
            hide_automation_workspaces: stored_hide_automation_workspaces(&legacy),
            // No legacy spelling to migrate: both filters are new here.
            hide_default_branch_workspaces: false,
            hide_detached_head_workspaces: false,
            confirm_close_pinned: stored_confirm_close_pinned(&legacy),
            diff_side_by_side: stored_diff_side_by_side(&legacy),
            window_material: stored_window_material(&legacy),
            default_agent: stored_default_agent(&legacy),
            agent_teams_mode: stored_teams_mode(&legacy),
            worktree_prefs: stored_worktree_prefs(&legacy),
            ..Self::default()
        }
        .normalized()
    }

    pub(super) fn normalized(mut self) -> Self {
        if !LOCALES.contains(&self.locale.as_str()) {
            self.locale = "system".to_string();
        }
        if !THEMES.contains(&self.theme.as_str()) {
            self.theme = "system".to_string();
        }
        self.ui_zoom_level = normalize_zoom_level(self.ui_zoom_level);
        if self.vault_session_limit != 0 {
            self.vault_session_limit =
                zerocode_core::vault::Limits::DEFAULT.session_limit(Some(self.vault_session_limit));
        }
        self.app_font_family = normalized_app_font_family(&self.app_font_family);
        adopt_unseen_status_bar_items(&mut self.status_bar_items, &mut self.status_bar_items_seen);
        self.left_sidebar_tint_color =
            normalized_left_sidebar_tint_color(&self.left_sidebar_tint_color);
        self.left_sidebar_tint_opacity =
            normalized_left_sidebar_tint_opacity(self.left_sidebar_tint_opacity);
        self.panel_widths = self.panel_widths.clamped();
        self.editing_prefs = self.editing_prefs.clamped();
        self.update = self.update.normalized();
        self.terminal_prefs = self.terminal_prefs.clamped();
        self.window_material = self.window_material.clamped();
        self.workspace_creation_prefs = self.workspace_creation_prefs.normalized();
        self.floating_workspace = self.floating_workspace.normalized();
        self.open_in_applications = normalize_open_in_applications(self.open_in_applications);
        self.default_agent = self.default_agent.validated().unwrap_or_default();
        if self.agent_teams_mode == TeamsMode::Panes && cfg!(not(unix)) {
            self.agent_teams_mode = TeamsMode::InProcess;
        }
        self.hidden_shortcuts
            .retain(|name| SHORTCUTS.contains(&name.as_str()));
        self.hidden_shortcuts.sort();
        self.hidden_shortcuts.dedup();
        self.hidden_task_sources
            .retain(|name| TASK_SOURCES.contains(&name.as_str()));
        self.hidden_task_sources.sort();
        self.hidden_task_sources.dedup();
        self.workspace_board = self.workspace_board.normalized();
        // Only sources with a working backend belong in the canonical
        // settings allowlist.
        // At least one must remain reachable, including after a stale-window
        // write or a hand-edited file.
        if TASK_SOURCES
            .iter()
            .all(|source| self.hidden_task_sources.iter().any(|row| row == source))
        {
            self.hidden_task_sources.retain(|row| row != "jira");
        }
        if !TASK_SOURCES.contains(&self.default_task_source.as_str())
            || self
                .hidden_task_sources
                .iter()
                .any(|row| row == &self.default_task_source)
        {
            self.default_task_source = TASK_SOURCES
                .iter()
                .copied()
                .find(|source| !self.hidden_task_sources.iter().any(|row| row == source))
                .unwrap_or("jira")
                .to_string();
        }
        self.browser.home_page =
            normalized_browser_url(&self.browser.home_page).unwrap_or_default();
        self.browser.default_zoom_level = normalize_zoom_level(self.browser.default_zoom_level);
        self.browser.open_tabs =
            self.browser
                .open_tabs
                .into_iter()
                .filter_map(|tab| {
                    normalized_browser_url(&tab.url).map(|url| StoredBrowserTab {
                        url,
                        // A dress is a short preset name, never free text.
                        viewport: tab.viewport.filter(|id| {
                            !id.is_empty()
                                && id.len() <= 24
                                && id.chars().all(|c| c.is_ascii_lowercase() || c == '-')
                        }),
                        mobile: tab.mobile,
                        // A reader is one printable line within the table's
                        // length — the bound a per-site row keeps.
                        reader: tab.reader.map(|reader| reader.trim().to_string()).filter(
                            |reader| {
                                !reader.is_empty()
                                    && reader.chars().count() <= BROWSER_USER_AGENT_MAX_CHARS
                                    && reader.chars().all(|c| (' '..='~').contains(&c))
                            },
                        ),
                        // A jar is only ever one of our minted ids.
                        profile: tab.profile.filter(|id| browser_profile_store(id).is_some()),
                    })
                })
                .take(32)
                .collect();
        self.ssh_hosts = ssh_hosts::normalize_entries(self.ssh_hosts);
        self.remote_workspaces =
            remote_workspaces::normalize_entries(self.remote_workspaces, &self.ssh_hosts);
        self.remote_servers = remote_servers::normalize_entries(self.remote_servers);
        self.ssh_targets = ssh_store::normalize_entries(self.ssh_targets);
        self.deleted_ssh_config_aliases =
            ssh_store::normalize_aliases(self.deleted_ssh_config_aliases);
        self.second_brain_vault = self.second_brain_vault.trim().to_string();
        self.google_account_email = self.google_account_email.trim().to_string();
        self
    }
}

pub(super) fn normalized_browser_url(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Some(String::new());
    }
    if raw == "about:blank" {
        return Some(raw.to_string());
    }
    let parsed = reqwest::Url::parse(raw).ok()?;
    matches!(parsed.scheme(), "http" | "https").then(|| parsed.to_string())
}

#[derive(Clone, Serialize)]
pub(super) struct SettingsSnapshot {
    pub(super) explorer_policy: explorer_policy::ExplorerPolicy,
    pub(super) schema_version: u32,
    pub(super) revision: u64,
    pub(super) system_locale: String,
    pub(super) terminal_opacity: f64,
    pub(super) window_blur: bool,
    pub(super) settings_health: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) settings_error: Option<String>,
    pub(super) editing_prefs_spec: EditingPrefsSpec,
    /// The update clock and the pickers' words (t-3191): the window arms its
    /// one boot timer from here and offers exactly these policies.
    pub(super) update_spec: UpdateSpec,
    pub(super) terminal_prefs_spec: TerminalPrefsSpec,
    pub(super) left_sidebar_appearance_spec: LeftSidebarAppearanceSpec,
    pub(super) ui_zoom_spec: ZoomLevelSpec,
    pub(super) open_in_applications_spec: OpenInApplicationsSpec,
    pub(super) browser_zoom_spec: ZoomLevelSpec,
    /// The retention field's bounds, read from the artifact table so the
    /// window never spells a second copy of them.
    pub(super) artifacts_retention_spec: U32SettingSpec,
    pub(super) vault_limits: zerocode_core::vault::Limits,
    /// The PR depth's numbers with the saved overlay laid over them (t-2733):
    /// the window polls and caches by these and spells none of its own.
    pub(super) checks_limits: zerocode_core::checks::Limits,
    /// The readiness snapshot's numbers with the saved overlay laid over
    /// them (t-3996), so the window can say how old a 「로그인 필요」 may be.
    pub(super) readiness_limits: zerocode_core::readiness::Limits,
    pub(super) crash_limits: crash::Limits,
    #[serde(flatten)]
    pub(super) document: SettingsDocument,
}

/// A numbers table's overlay, read off one object of the settings document:
/// `{"poll_ms": 6000}` under `checks` is `checks.poll_ms` to
/// `checks::Limits::overlaid`. Only whole numbers count; a word or a
/// fraction is not a limit. One reader for every table that takes one.
pub(super) fn u64_overlay(
    prefix: &str,
    overlay: &serde_json::Value,
) -> std::collections::BTreeMap<String, u64> {
    overlay
        .as_object()
        .map(|fields| {
            fields
                .iter()
                .filter_map(|(key, value)| Some((format!("{prefix}{key}"), value.as_u64()?)))
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn health_name(health: &settings::SettingsHealth) -> &'static str {
    match health {
        settings::SettingsHealth::Missing => "migrated",
        settings::SettingsHealth::Healthy => "healthy",
        settings::SettingsHealth::Recovered { .. } => "recovered",
    }
}

pub(super) fn snapshot_of(
    document: SettingsDocument,
    revision: u64,
    health: &'static str,
) -> SettingsSnapshot {
    SettingsSnapshot {
        explorer_policy: explorer_policy::ExplorerPolicy::overlay(&document.explorer),
        checks_limits: checks_runtime::limits_of(&document.checks),
        readiness_limits: readiness_runtime::limits_of(&document.readiness),
        crash_limits: crash::Limits::overlay(&document.crash),
        schema_version: SETTINGS_SCHEMA_VERSION,
        revision,
        system_locale: system_locale(),
        terminal_opacity: document.window_material.terminal_opacity,
        window_blur: document.window_material.blur,
        settings_health: health,
        settings_error: None,
        editing_prefs_spec: EditingPrefsSpec::default(),
        update_spec: UpdateSpec::default(),
        terminal_prefs_spec: TerminalPrefsSpec::default(),
        left_sidebar_appearance_spec: LeftSidebarAppearanceSpec::default(),
        ui_zoom_spec: ZoomLevelSpec::default(),
        open_in_applications_spec: OpenInApplicationsSpec::default(),
        browser_zoom_spec: ZoomLevelSpec::default(),
        artifacts_retention_spec: U32SettingSpec {
            min: 0,
            max: artifact_runtime::retention_days_max(),
            step: 1,
        },
        vault_limits: zerocode_core::vault::Limits::DEFAULT,
        document,
    }
}

pub(super) fn settings_migration_seed(
    repository: &settings::SettingsRepository,
) -> Result<SettingsDocument, String> {
    if legacy_migration_completed(repository, LegacyMigration::SettingsDocument)? {
        Ok(SettingsDocument::default())
    } else {
        Ok(SettingsDocument::from_legacy(repository.root()))
    }
}

/// Finish the two-file migration transaction after the canonical document is
/// durable. Marker failures are surfaced without discarding or overwriting the
/// canonical value that was already read or committed.
pub(super) fn record_settings_migration(
    repository: &settings::SettingsRepository,
    mut snapshot: SettingsSnapshot,
) -> SettingsSnapshot {
    if let Err(marker_error) =
        complete_legacy_migration(repository, LegacyMigration::SettingsDocument)
    {
        snapshot.settings_health = "error";
        snapshot.settings_error = Some(match snapshot.settings_error.take() {
            Some(error) => format!("{error}; could not record settings migration: {marker_error}"),
            None => format!("could not record settings migration: {marker_error}"),
        });
    }
    snapshot
}

pub(super) fn load_settings(
    repository: &settings::SettingsRepository,
) -> Result<SettingsSnapshot, String> {
    let read = repository
        .read_json::<SettingsDocument>(SETTINGS_DOCUMENT_FILE)
        .map_err(|error| error.to_string())?;
    let needs_commit = read
        .value
        .as_ref()
        .is_none_or(|document| document.clone().normalized() != *document);
    if needs_commit {
        let seed = settings_migration_seed(repository)?;
        // Re-read and normalize under the repository lock. Another process
        // may have committed after the read above; mutating that fresh value
        // prevents this load from overwriting it with a stale migration seed.
        let committed = repository
            .mutate_json(
                SETTINGS_DOCUMENT_FILE,
                || seed,
                |fresh| {
                    *fresh = fresh.clone().normalized();
                },
            )
            .map_err(|error| error.to_string())?;
        return Ok(record_settings_migration(
            repository,
            snapshot_of(
                committed.value,
                committed.revision,
                health_name(&committed.prior_health),
            ),
        ));
    }
    let document = read
        .value
        .expect("a commit is required when settings are missing");
    Ok(record_settings_migration(
        repository,
        snapshot_of(document, read.revision, health_name(&read.health)),
    ))
}

/// Read a settings snapshot without losing the first recovery error.
///
/// Both startup and a live settings refresh use this door. A corrupt primary
/// is quarantined by the repository; this call then seals a canonical default
/// while returning the original error once. The following read is healthy.
pub(super) fn load_settings_resilient(
    repository: &settings::SettingsRepository,
) -> SettingsSnapshot {
    load_settings(repository).unwrap_or_else(|error| {
        // Legacy files are an import source only when the canonical document
        // has never existed. Re-reading them after a corrupt canonical file
        // would roll the user back to an arbitrarily old preference set.
        // Recovery may have quarantined the unreadable primary. Seal that
        // state with a fresh canonical default so the next read cannot mistake
        // the now-missing primary for permission to run legacy migration. A
        // concurrent repair wins: the locked mutation normalizes that fresh
        // document instead of replacing it with our fallback.
        let (document, revision) = repository
            .mutate_json(SETTINGS_DOCUMENT_FILE, SettingsDocument::default, |fresh| {
                *fresh = fresh.clone().normalized()
            })
            .map_or((SettingsDocument::default(), 0), |committed| {
                (committed.value, committed.revision)
            });
        let mut snapshot = snapshot_of(document, revision, "error");
        snapshot.settings_error = Some(error);
        record_settings_migration(repository, snapshot)
    })
}

pub(super) fn load_settings_for_boot(
    repository: &settings::SettingsRepository,
) -> SettingsSnapshot {
    load_settings_resilient(repository)
}

pub(super) fn mutate_settings(
    repository: &settings::SettingsRepository,
    change: impl FnOnce(&mut SettingsDocument) -> Result<(), String>,
) -> Result<SettingsSnapshot, String> {
    let seed = settings_migration_seed(repository)?;
    let committed = repository
        .try_mutate_json(
            SETTINGS_DOCUMENT_FILE,
            || seed,
            |document| {
                *document = document.clone().normalized();
                change(document)?;
                *document = document.clone().normalized();
                Ok::<(), String>(())
            },
        )
        .map_err(|error| error.to_string())??;
    let _ = &committed.result;
    // Every pane this window opens from now on is told where the second brain
    // is, and `hooks::pty_env` has no state to ask — so the one funnel every
    // settings write passes through publishes it. Any road that sets the vault,
    // moves it, or clears it lands here, which is why this is not repeated at
    // the second-brain command.
    hooks::publish_second_brain_vault(&committed.value.second_brain_vault);
    hang_watchdog::configure(crash::Limits::overlay(&committed.value.crash));
    readiness_runtime::configure(readiness_runtime::limits_of(&committed.value.readiness));
    Ok(record_settings_migration(
        repository,
        snapshot_of(
            committed.value,
            committed.revision,
            health_name(&committed.prior_health),
        ),
    ))
}

#[derive(Clone, Serialize)]
pub(super) struct SettingsChanged {
    pub(super) revision: u64,
    pub(super) keys: Vec<String>,
}

pub(super) fn emit_settings_changed(app: &AppHandle, origin: &str, revision: u64, keys: &[&str]) {
    let event = SettingsChanged {
        revision,
        keys: keys.iter().map(|key| (*key).to_string()).collect(),
    };
    for label in [MAIN_WINDOW_LABEL, BOARD_POPOUT_LABEL] {
        if label != origin && app.get_webview_window(label).is_some() {
            let _ = app.emit_to(label, SETTINGS_CHANGED_EVENT, &event);
        }
    }
}

pub(super) fn emit_ssh_link(app: &AppHandle, report: &ssh_link::LinkReport) {
    if matches!(
        report.state,
        ssh_link::LinkState::Connected | ssh_link::LinkState::Disconnected
    ) {
        let app = app.clone();
        let id = format!("{}{}", sftp_runtime::TARGET_PREFIX, report.id);
        let connected = report.state == ssh_link::LinkState::Connected;
        let target_id = report.id.clone();
        tauri::async_runtime::spawn(async move {
            let service = app.state::<sftp_runtime::SftpService>();
            if connected {
                let still_connected = app
                    .state::<AppState>()
                    .ssh_links()
                    .get(&target_id)
                    .is_some_and(|r| r.state == ssh_link::LinkState::Connected);
                if !still_connected {
                    return;
                }
                if let Err(error) = service.connect(&app, &id).await {
                    let _ = app.emit(
                        "sftp:connection",
                        sftp_runtime::ConnectionReport {
                            host_id: id,
                            state: "error",
                            home: None,
                            error: Some(error),
                        },
                    );
                }
            } else {
                service.disconnect_host(&app, &id).await;
            }
        });
    }

    for label in [MAIN_WINDOW_LABEL, BOARD_POPOUT_LABEL] {
        if app.get_webview_window(label).is_some() {
            let _ = app.emit_to(label, SSH_LINK_EVENT, report);
        }
    }
}

/// How often a standing master is asked whether it still stands. `-O check`
/// is local socket talk — a heartbeat, not a network probe.
pub(super) const LINK_WATCH_INTERVAL_SECONDS: u64 = 15;

pub(super) fn link_epoch(app: &AppHandle, id: &str) -> u64 {
    let state = app.state::<AppState>();
    let held = state.ssh_link_epochs();
    held.get(id).copied().unwrap_or(0)
}

/// Write a report into the book and broadcast it — but only while `epoch`
/// still owns the id. Answers whether it did; a `false` means a newer
/// intention took over and this writer's story is finished.
pub(super) fn finish_link(
    app: &AppHandle,
    id: &str,
    epoch: u64,
    report: ssh_link::LinkReport,
) -> bool {
    let state = app.state::<AppState>();
    {
        let epochs = state.ssh_link_epochs();
        if epochs.get(id).copied().unwrap_or(0) != epoch {
            return false;
        }
        state.ssh_links().insert(id.to_string(), report.clone());
    }
    emit_ssh_link(app, &report);
    true
}

/// Follow one connection for its whole life: watch the master while it
/// stands, climb the measured ladder when it falls, and retire the moment
/// the epoch says a newer intention owns the id. Ladder position is carried
/// across short-lived comebacks; only sixty stable seconds hand it back.
pub(super) fn spawn_link_watcher(
    app: AppHandle,
    id: String,
    target: ssh_store::SshTarget,
    epoch: u64,
) {
    if !target.system_ssh_connection_reuse {
        // A reuse-off "connected" is a memory with no master to watch.
        return;
    }
    tauri::async_runtime::spawn(async move {
        let mut rungs_spent: usize = 0;
        'standing: loop {
            let stood_at = std::time::Instant::now();
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(LINK_WATCH_INTERVAL_SECONDS))
                    .await;
                if link_epoch(&app, &id) != epoch {
                    return;
                }
                let probe = target.clone();
                let alive =
                    tauri::async_runtime::spawn_blocking(move || ssh_link::check_link(&probe))
                        .await
                        .unwrap_or(false);
                if !alive {
                    break;
                }
                if stood_at.elapsed().as_secs() >= ssh_link::STABLE_AFTER_SECONDS {
                    rungs_spent = 0;
                }
            }
            let mut first_of_run = true;
            loop {
                let Some(wait) = ssh_link::reconnect_wait(rungs_spent, first_of_run) else {
                    finish_link(
                        &app,
                        &id,
                        epoch,
                        ssh_link::LinkReport {
                            error: Some("재연결 시도 한도에 도달했습니다".to_string()),
                            ..ssh_link::LinkReport::new(
                                &id,
                                ssh_link::LinkState::ReconnectionFailed,
                            )
                        },
                    );
                    return;
                };
                let step = ssh_link::LinkReport {
                    attempt: Some(u32::try_from(rungs_spent + 1).unwrap_or(u32::MAX)),
                    ..ssh_link::LinkReport::new(&id, ssh_link::LinkState::Reconnecting)
                };
                if !finish_link(&app, &id, epoch, step) {
                    return;
                }
                tokio::time::sleep(std::time::Duration::from_secs(wait)).await;
                if link_epoch(&app, &id) != epoch {
                    return;
                }
                let dial = target.clone();
                let outcome =
                    tauri::async_runtime::spawn_blocking(move || ssh_link::open_link(&dial))
                        .await
                        .unwrap_or_else(|err| {
                            Err(ssh_link::LinkRefusal {
                                auth: false,
                                transient: false,
                                said: err.to_string(),
                            })
                        });
                rungs_spent += 1;
                first_of_run = false;
                match outcome {
                    Ok(()) => {
                        if !finish_link(
                            &app,
                            &id,
                            epoch,
                            ssh_link::LinkReport::new(&id, ssh_link::LinkState::Connected),
                        ) {
                            return;
                        }
                        continue 'standing;
                    }
                    Err(fall) if fall.transient => {}
                    Err(fall) => {
                        let state = if fall.auth {
                            ssh_link::LinkState::AuthFailed
                        } else {
                            ssh_link::LinkState::Error
                        };
                        finish_link(
                            &app,
                            &id,
                            epoch,
                            ssh_link::LinkReport {
                                error: Some(fall.said),
                                ..ssh_link::LinkReport::new(&id, state)
                            },
                        );
                        return;
                    }
                }
            }
        }
    });
}

/// The asker is a WEBVIEW, not a `WebviewWindow`: the moment a browser pane
/// is added the window carries several webviews, and Tauri's
/// `WebviewWindow` extractor refuses every invoke with "current webview is
/// not a WebviewWindow" (live report 2026-08-14 — settings quietly died
/// whenever a browser tab existed). The label is all this ever needed.
pub(super) fn commit_setting(
    app: &AppHandle,
    webview: &tauri::Webview,
    state: &State<'_, AppState>,
    keys: &[&str],
    change: impl FnOnce(&mut SettingsDocument) -> Result<(), String>,
) -> Result<SettingsSnapshot, String> {
    let snapshot = mutate_settings(state.settings(), change)?;
    emit_settings_changed(app, webview.label(), snapshot.revision, keys);
    Ok(snapshot)
}

pub(super) fn validate_locale(code: &str) -> Result<(), String> {
    LOCALES
        .contains(&code)
        .then_some(())
        .ok_or_else(|| format!("{code}는 이 창이 아는 언어가 아닙니다"))
}

pub(super) fn validate_theme(code: &str) -> Result<(), String> {
    THEMES
        .contains(&code)
        .then_some(())
        .ok_or_else(|| format!("{code}는 이 창이 아는 테마가 아닙니다"))
}

pub(super) fn validate_shortcut_rows(rows: &[String]) -> Result<(), String> {
    match rows.iter().find(|name| !SHORTCUTS.contains(&name.as_str())) {
        Some(unknown) => Err(format!("{unknown}는 이 창에 있는 줄이 아닙니다")),
        None => Ok(()),
    }
}

pub(super) fn validate_default_agent(
    preference: zerocode_core::DefaultAgentPreference,
) -> Result<zerocode_core::DefaultAgentPreference, String> {
    let invalid_id = preference.agent_id().unwrap_or_default().to_string();
    preference
        .validated()
        .ok_or_else(|| format!("{invalid_id}은(는) 이 창이 아는 에이전트가 아닙니다"))
}

#[tauri::command(async)]
pub(super) fn settings_snapshot(state: State<'_, AppState>) -> SettingsSnapshot {
    load_settings_resilient(state.settings())
}

pub(super) fn ssh_hosts_report(
    repository: &settings::SettingsRepository,
    service: &ssh_hosts::SshHostService,
) -> Result<ssh_hosts::SshHostsReport, String> {
    let snapshot = load_settings_resilient(repository);
    service
        .report(&snapshot.document.ssh_hosts)
        .map_err(|error| error.to_string())
}

pub(super) fn save_ssh_host_transaction(
    repository: &settings::SettingsRepository,
    service: &ssh_hosts::SshHostService,
    input: ssh_hosts::SshHostInput,
) -> Result<(SettingsSnapshot, ssh_hosts::SshHostsReport), String> {
    let prepared = input.prepare().map_err(|error| error.to_string())?;
    let id = prepared.entry.id();
    let current = load_settings_resilient(repository);
    let existing = current
        .document
        .ssh_hosts
        .iter()
        .find(|entry| entry.id() == id);
    if prepared.update && existing.is_none() {
        return Err(ssh_hosts::SshHostsError::HostNotFound.to_string());
    }

    if prepared.entry.record().authentication() == zerocode_core::host::SshAuthentication::Password
        && prepared.password.is_none()
    {
        match existing.map(|entry| service.credential_status(entry.record())) {
            Some(ssh_hosts::SshCredentialStatus::Available) => {}
            Some(ssh_hosts::SshCredentialStatus::Unreadable) => {
                return Err(ssh_hosts::SshHostsError::CredentialUnavailable.to_string());
            }
            _ => return Err(ssh_hosts::SshHostsError::PasswordRequired.to_string()),
        }
    }

    if prepared.entry.record().authentication() == zerocode_core::host::SshAuthentication::Agent {
        service
            .delete_password(id)
            .map_err(|error| error.to_string())?;
    }

    let entry = prepared.entry;
    let update = prepared.update;
    let snapshot = mutate_settings(repository, move |document| {
        ssh_hosts::upsert_entry(&mut document.ssh_hosts, entry, update)
            .map_err(|error| error.to_string())
    })?;
    if let Some(password) = prepared.password {
        service
            .store_password(id, password)
            .map_err(|error| error.to_string())?;
    }
    let report = service
        .report(&snapshot.document.ssh_hosts)
        .map_err(|error| error.to_string())?;
    Ok((snapshot, report))
}

pub(super) fn remove_ssh_host_transaction(
    repository: &settings::SettingsRepository,
    service: &ssh_hosts::SshHostService,
    id: &str,
) -> Result<(SettingsSnapshot, ssh_hosts::SshHostsReport), String> {
    let id = ssh_hosts::parse_id(id).map_err(|error| error.to_string())?;
    let current = load_settings_resilient(repository);
    ssh_hosts::find_entry(&current.document.ssh_hosts, id).map_err(|error| error.to_string())?;
    service
        .delete_password(id)
        .map_err(|error| error.to_string())?;
    let snapshot = mutate_settings(repository, move |document| {
        ssh_hosts::remove_entry(&mut document.ssh_hosts, id)
            .map(|_| remote_workspaces::remove_for_host(&mut document.remote_workspaces, id))
            .map_err(|error| error.to_string())
    })?;
    let report = service
        .report(&snapshot.document.ssh_hosts)
        .map_err(|error| error.to_string())?;
    Ok((snapshot, report))
}

/* ---- SSH targets ----------------------------------------------------------
 *
 * The settings-domain CRUD for Orca's SSH host list. Nothing here connects:
 * a target is a description of a machine, and the connection lifecycle
 * (1-g55c) is its own domain with its own state. The three commands are the
 * whole surface — list, insert-or-update, remove — and every rule about what
 * a target may contain lives in `ssh_store`, so a renderer that skipped its
 * own checks still cannot write one this window could not open. */
pub(super) fn ssh_targets_list(
    repository: &settings::SettingsRepository,
) -> Vec<ssh_store::SshTarget> {
    load_settings_resilient(repository).document.ssh_targets
}

pub(super) fn save_ssh_target_transaction(
    repository: &settings::SettingsRepository,
    target: ssh_store::SshTarget,
) -> Result<(SettingsSnapshot, Vec<ssh_store::SshTarget>), String> {
    let prepared = target.prepare().map_err(|error| error.to_string())?;
    let target = prepared.target;
    let update = prepared.update;
    let snapshot = mutate_settings(repository, move |document| {
        ssh_store::upsert_entry(&mut document.ssh_targets, target, update)
            .map_err(|error| error.to_string())
    })?;
    let targets = snapshot.document.ssh_targets.clone();
    Ok((snapshot, targets))
}

pub(super) fn remove_ssh_target_transaction(
    repository: &settings::SettingsRepository,
    id: &str,
) -> Result<(SettingsSnapshot, Vec<ssh_store::SshTarget>), String> {
    let id = ssh_store::parse_id(id).map_err(|error| error.to_string())?;
    let snapshot = mutate_settings(repository, move |document| {
        // Remembered in the same transaction that removes it: a deletion
        // recorded afterwards is a window in which the silent sync paints the
        // card straight back.
        ssh_store::remove_entry(&mut document.ssh_targets, &id)
            .map(|removed| {
                ssh_store::record_removed_alias(&mut document.deleted_ssh_config_aliases, &removed);
            })
            .map_err(|error| error.to_string())
    })?;
    let targets = snapshot.document.ssh_targets.clone();
    Ok((snapshot, targets))
}

/// Fold `~/.ssh/config` into the list, and report only what moved.
///
/// `re_adopt` is the difference between the two ways this runs. The pane
/// calls it silently every time it opens and the suppression list stands; the
/// 가져오기 button calls it with the amnesty, which forgives every alias
/// somebody deleted and lets the file speak for them again.
pub(super) fn import_ssh_config_transaction(
    repository: &settings::SettingsRepository,
    re_adopt: bool,
) -> Result<(SettingsSnapshot, usize), String> {
    let parsed = ssh_config::read_user_config().map_err(|error| error.to_string())?;
    let current = load_settings_resilient(repository);

    // Rehearsed before it is written. This runs on every pane open, and a
    // document rewritten each time is a revision bump and a backup rotation
    // for a file nobody changed. The rehearsal decides only WHETHER to write;
    // the count that is reported comes from the write itself, under the lock.
    let mut suppressed = current.document.deleted_ssh_config_aliases.clone();
    let forgiving = re_adopt && !suppressed.is_empty();
    if re_adopt {
        suppressed.clear();
    }
    let mut rehearsal = current.document.ssh_targets.clone();
    if ssh_store::sync_from_config(&mut rehearsal, &suppressed, &parsed) == 0 && !forgiving {
        return Ok((current, 0));
    }

    let mut changed = 0;
    let snapshot = mutate_settings(repository, |document| {
        if re_adopt {
            document.deleted_ssh_config_aliases.clear();
        }
        changed = ssh_store::sync_from_config(
            &mut document.ssh_targets,
            &document.deleted_ssh_config_aliases,
            &parsed,
        );
        Ok(())
    })?;
    Ok((snapshot, changed))
}

pub(super) fn ssh_connection_material(
    repository: &settings::SettingsRepository,
    service: &ssh_hosts::SshHostService,
    id: &str,
) -> Result<
    (
        zerocode_core::host::SshHostRecord,
        Option<zerocode_ssh::SshPassword>,
    ),
    String,
> {
    let id = ssh_hosts::parse_id(id).map_err(|error| error.to_string())?;
    let snapshot = load_settings_resilient(repository);
    ssh_connection_material_for_id(&snapshot.document, service, id)
}

pub(super) fn ssh_connection_material_for_id(
    document: &SettingsDocument,
    service: &ssh_hosts::SshHostService,
    id: zerocode_core::host::ExecutionHostId,
) -> Result<
    (
        zerocode_core::host::SshHostRecord,
        Option<zerocode_ssh::SshPassword>,
    ),
    String,
> {
    let record = ssh_hosts::find_entry(&document.ssh_hosts, id)
        .map_err(|error| error.to_string())?
        .record()
        .clone();
    let password = service
        .password(&record)
        .map_err(|error| error.to_string())?;
    Ok((record, password))
}

pub(super) fn remote_workspaces_report(
    repository: &settings::SettingsRepository,
) -> remote_workspaces::RemoteWorkspacesReport {
    let snapshot = load_settings_resilient(repository);
    remote_workspaces::report(
        &snapshot.document.remote_workspaces,
        &snapshot.document.ssh_hosts,
    )
}

pub(super) fn prepare_remote_workspace_save(
    repository: &settings::SettingsRepository,
    service: &ssh_hosts::SshHostService,
    input: remote_workspaces::RemoteWorkspaceInput,
) -> Result<
    (
        remote_workspaces::PreparedRemoteWorkspace,
        zerocode_core::host::SshHostRecord,
        Option<zerocode_ssh::SshPassword>,
    ),
    String,
> {
    let prepared = input.prepare().map_err(|error| error.to_string())?;
    let snapshot = load_settings_resilient(repository);
    if prepared.update {
        remote_workspaces::find_entry(&snapshot.document.remote_workspaces, prepared.entry.id())
            .map_err(|error| error.to_string())?;
    }
    let (record, password) =
        ssh_connection_material_for_id(&snapshot.document, service, prepared.entry.host_id())?;
    Ok((prepared, record, password))
}

pub(super) fn remote_workspace_connection_material(
    repository: &settings::SettingsRepository,
    service: &ssh_hosts::SshHostService,
    id: &str,
) -> Result<
    (
        remote_workspaces::RemoteWorkspaceEntry,
        zerocode_core::host::SshHostRecord,
        Option<zerocode_ssh::SshPassword>,
    ),
    String,
> {
    let id = remote_workspaces::parse_id(id).map_err(|error| error.to_string())?;
    let snapshot = load_settings_resilient(repository);
    let entry = remote_workspaces::find_entry(&snapshot.document.remote_workspaces, id)
        .map_err(|error| error.to_string())?
        .clone();
    let (record, password) =
        ssh_connection_material_for_id(&snapshot.document, service, entry.host_id())?;
    Ok((entry, record, password))
}

pub(super) fn persist_remote_workspace(
    repository: &settings::SettingsRepository,
    entry: remote_workspaces::RemoteWorkspaceEntry,
    update: bool,
) -> Result<(SettingsSnapshot, remote_workspaces::RemoteWorkspacesReport), String> {
    let snapshot = mutate_settings(repository, move |document| {
        remote_workspaces::upsert_entry(
            &mut document.remote_workspaces,
            &document.ssh_hosts,
            entry,
            update,
        )
        .map_err(|error| error.to_string())
    })?;
    let report = remote_workspaces::report(
        &snapshot.document.remote_workspaces,
        &snapshot.document.ssh_hosts,
    );
    Ok((snapshot, report))
}

pub(super) fn remove_remote_workspace_transaction(
    repository: &settings::SettingsRepository,
    id: &str,
) -> Result<(SettingsSnapshot, remote_workspaces::RemoteWorkspacesReport), String> {
    let id = remote_workspaces::parse_id(id).map_err(|error| error.to_string())?;
    let snapshot = mutate_settings(repository, move |document| {
        remote_workspaces::remove_entry(&mut document.remote_workspaces, id)
            .map(|_| ())
            .map_err(|error| error.to_string())
    })?;
    let report = remote_workspaces::report(
        &snapshot.document.remote_workspaces,
        &snapshot.document.ssh_hosts,
    );
    Ok((snapshot, report))
}

pub(super) fn remote_servers_report(
    repository: &settings::SettingsRepository,
    service: &remote_servers::RemoteServerService,
) -> remote_servers::RemoteServersReport {
    let snapshot = load_settings_resilient(repository);
    service.report(&snapshot.document.remote_servers)
}

pub(super) fn optional_remote_server_token(
    service: &remote_servers::RemoteServerService,
    id: uuid::Uuid,
) -> Result<Option<Zeroizing<String>>, String> {
    match service.token(id) {
        Ok(token) => Ok(Some(token)),
        Err(remote_servers::RemoteServerError::TokenRequired) => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

pub(super) fn restore_remote_server_token(
    service: &remote_servers::RemoteServerService,
    id: uuid::Uuid,
    previous: Option<Zeroizing<String>>,
) -> Result<(), String> {
    match previous {
        Some(token) => service.store_token(id, token),
        None => service.delete_token(id),
    }
    .map_err(|_| "remote server credential rollback failed".to_string())
}

pub(super) fn verify_remote_server(
    entry: &remote_servers::RemoteServerEntry,
    token: &str,
) -> Result<(), String> {
    match zerocode_lane::probe_session_server(entry.endpoint(), token) {
        ServeProbe::Ready => Ok(()),
        ServeProbe::NotListening => {
            Err("remote server is not listening through the loopback tunnel".to_string())
        }
        ServeProbe::NotSessionServer => {
            Err("tunnel endpoint is not a ZeroCode session server".to_string())
        }
        ServeProbe::Unauthorized => Err("remote server rejected the access token".to_string()),
    }
}

pub(super) fn save_remote_server_transaction(
    repository: &settings::SettingsRepository,
    service: &remote_servers::RemoteServerService,
    input: remote_servers::RemoteServerInput,
) -> Result<(SettingsSnapshot, remote_servers::RemoteServersReport), String> {
    let prepared = input.prepare().map_err(|error| error.to_string())?;
    let id = prepared.entry.id();
    let current = load_settings_resilient(repository);
    if prepared.update {
        remote_servers::find_entry(&current.document.remote_servers, id)
            .map_err(|error| error.to_string())?;
    }
    verify_remote_server(&prepared.entry, prepared.token.as_str())?;

    let previous = optional_remote_server_token(service, id)?;
    service
        .store_token(id, prepared.token)
        .map_err(|error| error.to_string())?;
    let entry = prepared.entry;
    let update = prepared.update;
    let snapshot = match mutate_settings(repository, move |document| {
        remote_servers::upsert_entry(&mut document.remote_servers, entry, update)
            .map_err(|error| error.to_string())
    }) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            restore_remote_server_token(service, id, previous)?;
            return Err(error);
        }
    };
    let report = service.report(&snapshot.document.remote_servers);
    Ok((snapshot, report))
}

pub(super) fn remove_remote_server_transaction(
    repository: &settings::SettingsRepository,
    service: &remote_servers::RemoteServerService,
    id: &str,
) -> Result<(SettingsSnapshot, remote_servers::RemoteServersReport), String> {
    let id = remote_servers::parse_id(id).map_err(|error| error.to_string())?;
    let current = load_settings_resilient(repository);
    remote_servers::find_entry(&current.document.remote_servers, id)
        .map_err(|error| error.to_string())?;
    let previous = optional_remote_server_token(service, id)?;
    service
        .delete_token(id)
        .map_err(|error| error.to_string())?;
    let snapshot = match mutate_settings(repository, move |document| {
        remote_servers::remove_entry(&mut document.remote_servers, id)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            restore_remote_server_token(service, id, previous)?;
            return Err(error);
        }
    };
    let report = service.report(&snapshot.document.remote_servers);
    Ok((snapshot, report))
}

pub(super) fn remote_server_connection_material(
    repository: &settings::SettingsRepository,
    service: &remote_servers::RemoteServerService,
    id: &str,
) -> Result<(remote_servers::RemoteServerEntry, Zeroizing<String>), String> {
    let id = remote_servers::parse_id(id).map_err(|error| error.to_string())?;
    let snapshot = load_settings_resilient(repository);
    let entry = remote_servers::find_entry(&snapshot.document.remote_servers, id)
        .map_err(|error| error.to_string())?
        .clone();
    let token = service.token(id).map_err(|error| error.to_string())?;
    Ok((entry, token))
}

/// Opens the credential modal and waits — [`ssh_prompt::CREDENTIAL_TIMEOUT`]
/// at most (P0-20). The request travels as Orca's own event pair
/// (`ssh:credential-request` / `ssh:credential-resolved`,
/// `ssh-passphrase.ts:14-48`); an answered question is resolved by the
/// submit command, and a timed-out or orphaned one is withdrawn HERE,
/// exactly once, so the modal never stands over a question nobody is still
/// asking.
pub(super) async fn ask_ssh_credential(
    app: &AppHandle,
    target_id: &str,
    kind: ssh_prompt::CredentialKind,
    detail: &str,
) -> Option<String> {
    let request_id = uuid::Uuid::new_v4().to_string();
    let receiver = app.state::<AppState>().ssh_credentials().begin(&request_id);
    let _ = app.emit_to(
        MAIN_WINDOW_LABEL,
        "ssh:credential-request",
        ssh_prompt::CredentialRequest {
            request_id: request_id.clone(),
            target_id: target_id.to_string(),
            kind,
            detail: detail.to_string(),
        },
    );
    match tokio::time::timeout(ssh_prompt::CREDENTIAL_TIMEOUT, receiver).await {
        Ok(Ok(answer)) => answer,
        _ => {
            if app
                .state::<AppState>()
                .ssh_credentials()
                .abandon(&request_id)
            {
                let _ = app.emit_to(
                    MAIN_WINDOW_LABEL,
                    "ssh:credential-resolved",
                    ssh_prompt::CredentialResolved { request_id },
                );
            }
            None
        }
    }
}

/// One SSH connect, with the person on call (P0-20).
///
/// The connector speaks first; only when it fails FOR WANT OF A SECRET does
/// a modal open — one question, one retry, and a declined or timed-out
/// question surfaces the error the connect already had. That is Orca's own
/// shape: prompt once per kind per attempt (`ssh-connection.ts:816-857`).
/// Three wants qualify:
///
/// - a password host with nothing stored (`PasswordRequired`);
/// - a password host whose STORED password the server refused
///   (`AuthenticationRejected`) — the keychain is stale, and the person may
///   know the new password right now;
/// - an agent host whose agent cannot answer (`AgentUnavailable` /
///   `AgentSigning`) — "an agent socket failure can still be recovered by
///   password auth" (`ssh-connection.ts:845`), so the retry flips the same
///   verified record to password authentication, pinned key intact.
///
/// The prompted secret lives exactly as long as the retry: handed to
/// [`zerocode_ssh::SshPassword`], zeroized when the attempt returns, and
/// never persisted — Orca never writes an SSH secret either, it caches in
/// memory at most.
pub(super) async fn connect_with_prompts(
    app: &AppHandle,
    record: zerocode_core::host::SshHostRecord,
    password: Option<zerocode_ssh::SshPassword>,
) -> Result<zerocode_ssh::SshConnection, String> {
    use zerocode_ssh::ConnectError;
    let stored = password.is_some();
    let error = match zerocode_ssh::SshConnector::default()
        .connect(&record, password)
        .await
    {
        Ok(connection) => return Ok(connection),
        Err(error) => error,
    };
    let record = match &error {
        ConnectError::PasswordRequired => record,
        ConnectError::AuthenticationRejected if stored => record,
        ConnectError::AgentUnavailable { .. } | ConnectError::AgentSigning { .. } => {
            record.with_password_authentication()
        }
        _ => return Err(error.to_string()),
    };
    let detail = {
        let endpoint = record.endpoint();
        format!(
            "{}@{}:{}",
            endpoint.user(),
            endpoint.host(),
            endpoint.port()
        )
    };
    let Some(secret) = ask_ssh_credential(
        app,
        &record.id().to_string(),
        ssh_prompt::CredentialKind::Password,
        &detail,
    )
    .await
    else {
        return Err(error.to_string());
    };
    zerocode_ssh::SshConnector::default()
        .connect(&record, Some(zerocode_ssh::SshPassword::new(secret)))
        .await
        .map_err(|error| error.to_string())
}

pub(super) async fn verify_remote_workspace_root(
    app: &AppHandle,
    record: zerocode_core::host::SshHostRecord,
    password: Option<zerocode_ssh::SshPassword>,
    root: zerocode_core::host::RemotePath,
) -> Result<zerocode_core::host::RemotePath, String> {
    let mut connection = connect_with_prompts(app, record, password).await?;
    let verified = connection.verify_workspace_root(&root).await;
    let disconnected = connection.disconnect().await;
    match verified {
        Err(error) => Err(error.to_string()),
        Ok(root) => {
            disconnected.map_err(|error| error.to_string())?;
            Ok(root)
        }
    }
}

pub(super) async fn hold_shared_ssh_terminal(
    app: &AppHandle,
    connection: Arc<zerocode_ssh::SshConnection>,
    root: zerocode_core::host::RemotePath,
    rows: u16,
    cols: u16,
) -> Result<TermId, String> {
    let spec =
        zerocode_core::host::PtySpec::remote("sh", &["-l".to_string()], root, &[], rows, cols);
    let pty = connection
        .open_shared_pty(&spec)
        .await
        .map_err(|e| e.to_string())?;
    Ok(hold_terminal_handle(app, pty))
}

pub(super) fn hold_terminal_handle(app: &AppHandle, pty: PtyHandle) -> TermId {
    let state = app.state::<AppState>();
    let term = state.take_term_id();
    state.hold_terminal(term, pty);
    state.cadence().wake();
    term
}

pub(super) async fn run_remote_host_task<T, F>(task: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|_| "SSH host operation stopped unexpectedly".to_string())?
}

/// What a sync did, and nothing else. What the targets ARE is `ssh_targets`'s
/// answer — one command per question keeps the renderer from repainting a
/// list out of a reply it did not ask for.
#[derive(Serialize)]
pub(super) struct SshImportReport {
    pub(super) changed: usize,
}

/// `system:resumed`'s one fact — how long the lights were out. The window
/// re-checks SSH links on hearing it; the number is for the person reading
/// the husk beside it.
#[derive(Clone, Serialize)]
pub(super) struct SystemResumed {
    #[serde(rename = "sleptMs")]
    pub(super) slept_ms: u64,
}

/// 마지막으로 울린 벨의 주소 (P0-14) — 발사한 워크트리와, 있다면 그 판.
///
/// Orca의 알림 클릭은 `ui:activateWorktree`+`ui:focusTerminal`로 정확한
/// 판까지 걷는다(`native-notification-delivery.ts:74-108`). 이쪽 데스크톱
/// 알림 백엔드(notify-rust)는 클릭을 보고하지 않으므로 — 플러그인의
/// `actionPerformed`는 iOS·Android 구현뿐(2026-08-18 실측) — macOS가
/// 보장하는 "클릭이면 앱 활성화"를 근사의 발판으로 쓴다: `RunEvent::Reopen`.
#[derive(Clone, Serialize)]
pub(super) struct LastRing {
    pub(super) worktree: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) term: Option<TermId>,
    #[serde(skip)]
    pub(super) at: i64,
}

/// Reopen이 벨의 초대로 읽히는 시간 창.
///
/// 배너가 걷힌 뒤 알림 센터에서 눌러 오는 손은 맞아야 하고, 벨과 무관한
/// 아침의 dock 클릭이 어젯밤의 워크트리로 끌려가서는 안 된다 — 실클릭을
/// 듣는 Orca에는 없는 저울이다. 분 단위는 이 리포가 "사람이 이어서
/// 행동하는 한 호흡"에 쓰는 눈금(`MIN_REPORTABLE_SUSPEND`,
/// `STRAY_JAMO_REPORT_EVERY_MS`와 같은 60초)과 정렬했다.
pub(super) const RING_CLICK_WINDOW_MS: i64 = 60_000;

/// `(mode, active)` the way the wire speaks it — Orca's
/// `ComputerAwakeStatus` (`computer-awake-mode.ts`), read by the status
/// bar's caffeinate segment.
#[derive(Clone, Serialize)]
pub(super) struct AwakeStatus {
    pub(super) mode: awake::ComputerAwakeMode,
    pub(super) active: bool,
}

/// What the webview needs to draw its first frame.
#[derive(Serialize)]
pub(super) struct BootReport {
    /// Path to the discovered `zo`, or `None` — which the UI must render as a
    /// folded lane panel, not an error.
    pub(super) zo: Option<String>,
    pub(super) bind: String,
    /// `ready` / `unauthorized` / `foreign` / `down` — the four [`ServeProbe`]
    /// answers, pre-worded because the UI must not interpret protocol states.
    pub(super) serve: &'static str,
    /// Whether `zo` exists but its canonical token is unavailable. The full
    /// filesystem error stays in the backend log; the renderer localizes one
    /// bounded, credential-free explanation.
    pub(super) lane_error: bool,
    /// Titlebar identity: which project this window is open on…
    pub(super) project: String,
    /// …and which branch its checkout is sitting on, when that is knowable.
    pub(super) branch: Option<String>,
    pub(super) lanes: Vec<Lane>,
    pub(super) focused: Option<LaneId>,
    /// One canonical, revisioned preference snapshot is flattened into the
    /// first frame. `settings_snapshot` returns the exact same type later.
    #[serde(flatten)]
    pub(super) settings: SettingsSnapshot,
    /// What the window ACTUALLY got, which is not the same question. The
    /// material is asked for once at startup, so a switch flipped since then
    /// is a stored intent — Orca answers this with a "Requires restart"
    /// banner and so does the screen that reads this field.
    pub(super) window_blur_active: bool,
    /// 첫 실행 경험이 어디까지 왔나. 마법사를 띄울지는 `closed_at`이 비어
    /// 있는지 하나로 정해지고(1-ea), 그 판정이 **첫 프레임 전에** 서 있어야
    /// 한다 — 창을 다 그린 다음 전체화면 마법사가 덮으면, 그것은 사람이 이미
    /// 읽기 시작한 화면을 빼앗는 것이다.
    pub(super) onboarding: zerocode_core::Onboarding,
    pub(super) last_crash: Option<crash::LastCrash>,
}

/// The checked-out branch, read from `HEAD` without spawning git.
///
/// The fast reading, for the places that ask once per card behind a repaint
/// nobody requested. Anything that must survive a remote [`Host`] asks git
/// through the boundary instead ([`current_git_branch`]).
///
/// In a LINKED WORKTREE — which is most of what this application opens —
/// `.git` is not a directory but a file holding `gitdir: <path>`, and the
/// naive read of `<root>/.git/HEAD` answers "no branch" for every one of
/// them. That silence reached three surfaces: the commit-message prompt and
/// the pull-request draft both lost the branch name they were meant to be
/// written from, and the review lookup below could not tell one branch's
/// pull request from the next one's. The pointer is followed by
/// [`zerocode_core::git_dir::of`], which the conflict operation reads too —
/// one spelling of "where is this checkout's git directory", not two.
///
/// Best-effort otherwise, on purpose: a detached HEAD or no repository at all
/// are ordinary states, and a titlebar simply shows no branch rather than an
/// error.
pub(super) fn current_branch(root: &std::path::Path) -> Option<String> {
    let head = std::fs::read_to_string(zerocode_core::git_dir::of(root)?.join("HEAD")).ok()?;
    let name = head.trim().strip_prefix("ref: refs/heads/")?;
    (!name.is_empty()).then(|| name.to_string())
}

fn default_vault_session_limit() -> usize {
    zerocode_core::vault::Limits::DEFAULT.default
}

/// Half an hour with no pane on it before a kept-booted emulator is put away
/// (docs/design/emulator-first-second-20260921.md D3) — long enough to cover
/// a person looking at something else and coming back, short enough that a
/// device forgotten before lunch is not still holding its RAM after it.
const fn default_emulator_idle_shutdown_minutes() -> u32 {
    30
}
