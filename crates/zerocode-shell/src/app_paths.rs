//! Tauri's path resolver and the inventory that must move as one release.
//!
//! This module does not perform migration. It names every durable artifact and
//! classifies its destination; [`crate::state_migration`] owns the copy/receipt
//! transaction. Keeping the switch in `zerocode-lane` off until all legacy
//! callers are gone prevents a shell and CLI split-brain.

use std::path::PathBuf;

use tauri::Manager;
use zerocode_lane::legacy_state_root;

pub(crate) use zerocode_lane::{AppPaths, PathClass};

pub(crate) mod settings_file {
    pub(crate) use zerocode_lane::PREFERENCES_FILE as PREFERENCES;
    pub(crate) const PROJECT: &str = "project-settings.json";
    pub(crate) const AGENT_LAUNCH: &str = "agent-launch-settings.json";
    // Spelled once, where the store that reads it lives.
    pub(crate) use zerocode_shell_state::settings_file::WORK_ITEM_LINKS;
    pub(crate) const LEGACY_MIGRATIONS: &str = "legacy-migrations.json";

    pub(crate) const ALL: &[&str] = &[
        PREFERENCES,
        PROJECT,
        AGENT_LAUNCH,
        WORK_ITEM_LINKS,
        LEGACY_MIGRATIONS,
    ];
}

pub(crate) const SETTINGS_DOCUMENTS: &[&str] = settings_file::ALL;
const ACCOUNT_REPOSITORIES_MIGRATION_SCHEMA: &str = "account-repositories-transform-v1";
pub(crate) const CLAUDE_ACCOUNT_PATH_FIELD: &str = "config_dir";
pub(crate) const CODEX_ACCOUNT_PATH_FIELD: &str = "home_dir";

/// Runtime-owned artifact names that do not belong to a narrower domain
/// module. The migration manifest and every producer consume these constants;
/// changing a filename therefore cannot silently strand its previous value.
pub(crate) mod artifact_file {
    pub(crate) const LAUNCH_RECIPES: &str = "launch-recipes.json";
    pub(crate) const WORKSPACE_DISMISSALS: &str = "workspace-dismissals.json";
    pub(crate) const RECENT_PROJECTS: &str = "recent-projects.json";
    pub(crate) const LAST_WORKSPACE: &str = "last-workspace.json";
    pub(crate) const ONBOARDING: &str = "onboarding.json";
    pub(crate) const PANE_LAYOUTS: &str = "pane-layouts.json";
    pub(crate) const AUTOMATIONS: &str = "automations.json";
    pub(crate) const REPO_TRUST: &str = "repo-trust.json";
    pub(crate) const DEFAULT_TABS_APPLIED: &str = "default-tabs-applied.json";
    pub(crate) const QUICK_COMMANDS: &str = "quick-commands.json";
    pub(crate) const DIFF_NOTES: &str = "diff-notes.json";
    pub(crate) const STAGE_LAYOUTS: &str = "stage-layouts.json";
    pub(crate) const AUTOMATION_RUNS: &str = "automation-runs.json";
    pub(crate) const CLAUDE_USAGE: &str = "claude-usage.json";
    pub(crate) const CODEX_USAGE: &str = "codex-usage.json";
    pub(crate) const ANTIGRAVITY_USAGE: &str = "antigravity-usage.json";
    /// Kimi 게이지의 스냅샷 — 프로바이더마다 자기 파일이다(한 파일을 나눠
    /// 쓰면 한 CLI 의 수치가 다른 이름 아래 선다).
    pub(crate) const KIMI_USAGE: &str = "kimi-usage.json";
    pub(crate) const GROK_USAGE: &str = "grok-usage.json";
    pub(crate) const OPENCODE_USAGE: &str = "opencode-usage.json";
    pub(crate) const POPOUT_BOUNDS: &str = "popout-bounds.json";
    /// 메인 창이 마지막으로 있던 자리와 최대화 여부 (P0-16) — Orca는 이것을
    /// UI 스토어의 `windowBounds`/`windowMaximized` 한 쌍으로 들고 있다
    /// (`createMainWindow.ts:230,246,413-425`). 팝아웃과 **다른 파일**인 것이
    /// 요점이다: 두 창은 서로 다른 자리에 서 있고, 한 파일을 나눠 쓰면 나중에
    /// 닫힌 창이 먼저 닫힌 창의 자리를 덮는다.
    pub(crate) const MAIN_WINDOW_BOUNDS: &str = "main-window-bounds.json";
    /// 판마다의 마지막 소식, 재시작 너머로 (P0-13) — Orca의
    /// `last-status.json`(agent-hooks/server.ts:180)과 같은 이름, 같은 임무.
    pub(crate) const LAST_STATUS: &str = "last-status.json";
    pub(crate) const WEBVIEW_ERRORS: &str = "webview-errors.log";
    pub(crate) const SERVE_TOKEN_PREFIX: &str = zerocode_lane::SERVE_TOKEN_PREFIX;
    pub(crate) const CODEX_HOOK_BACKUP: &str = "hooks.json.pre-zerocode";
    /// The mirror home's first segment — the directory the migration moves.
    pub(crate) const CODEX_RUNTIME_HOME: &str =
        zerocode_core::codex_account::RUNTIME_HOME_SEGMENTS[0];
}

pub(crate) mod legacy_settings_file {
    pub(crate) const AGENT_LAUNCH: &str = "agent-launch.json";
    pub(crate) const AGENT_TEAMS_MODE: &str = "agent-teams-mode.json";
    pub(crate) const CONFIRM_CLOSE_PINNED: &str = "confirm-close-pinned.json";
    pub(crate) const DEFAULT_AGENT: &str = "default-agent.json";
    pub(crate) const DIFF_SIDE_BY_SIDE: &str = "diff-side-by-side.json";
    pub(crate) const HIDDEN_SHORTCUTS: &str = "hidden-shortcuts.json";
    pub(crate) const HIDDEN_TASK_SOURCES: &str = "hidden-task-sources.json";
    pub(crate) const HIDE_AUTOMATION_WORKSPACES: &str = "hide-automation-workspaces.json";
    pub(crate) const KEYBINDINGS: &str = "keybindings.json";
    pub(crate) const LOCALE: &str = "locale.json";
    pub(crate) const PANEL_WIDTHS: &str = "panel-widths.json";
    pub(crate) const PROJECT_SETTINGS: &str = "project-scripts.json";
    pub(crate) const TERMINAL_COMMAND: &str = "terminal-command.json";
    pub(crate) const TERMINAL_PREFS: &str = "terminal-prefs.json";
    pub(crate) const THEME: &str = "theme.json";
    pub(crate) const WINDOW_MATERIAL: &str = "window-material.json";
    pub(crate) const WORKTREE_PREFS: &str = "worktree-prefs.json";

    pub(crate) const ALL: &[&str] = &[
        AGENT_LAUNCH,
        AGENT_TEAMS_MODE,
        CONFIRM_CLOSE_PINNED,
        DEFAULT_AGENT,
        DIFF_SIDE_BY_SIDE,
        HIDDEN_SHORTCUTS,
        HIDDEN_TASK_SOURCES,
        HIDE_AUTOMATION_WORKSPACES,
        KEYBINDINGS,
        LOCALE,
        PANEL_WIDTHS,
        PROJECT_SETTINGS,
        TERMINAL_COMMAND,
        TERMINAL_PREFS,
        THEME,
        WINDOW_MATERIAL,
        WORKTREE_PREFS,
    ];
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArtifactLayout {
    File(&'static str),
    Directory(&'static str),
    Prefix(&'static str),
    /// Settings documents, revisions, backups, quarantine files, and the
    /// legacy split documents have to be reconciled as one logical store.
    SettingsRepository,
    /// Jira metadata may move only after pending transactions are recovered;
    /// credentials follow the native-vault migration, never a raw copy.
    JiraRepository,
    /// Account indexes and the managed homes named by them form one unit. The
    /// migration rewrites only app-owned legacy paths and removes the old
    /// credential directories after target readback succeeds.
    AccountRepositories,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MigrationKind {
    Copy,
    Transform,
    Recreate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ArtifactSpec {
    pub(crate) key: &'static str,
    pub(crate) class: PathClass,
    pub(crate) layout: ArtifactLayout,
    pub(crate) migration: MigrationKind,
}

/// Every app-owned artifact currently rooted below `~/.zerocode/state`.
///
/// External agent homes (`~/.claude`, `~/.codex`, agent plugin/config files)
/// are deliberately absent: the app may reconcile them, but does not own their
/// location. Managed account homes are present because the app created them.
pub(crate) const ARTIFACTS: &[ArtifactSpec] = &[
    ArtifactSpec {
        key: "settings-repository",
        class: PathClass::Config,
        layout: ArtifactLayout::SettingsRepository,
        migration: MigrationKind::Transform,
    },
    file(
        "launch-recipes",
        PathClass::Config,
        artifact_file::LAUNCH_RECIPES,
    ),
    file(
        "workspace-dismissals",
        PathClass::Config,
        artifact_file::WORKSPACE_DISMISSALS,
    ),
    file(
        "recent-projects",
        PathClass::Config,
        artifact_file::RECENT_PROJECTS,
    ),
    file(
        "last-workspace",
        PathClass::Config,
        artifact_file::LAST_WORKSPACE,
    ),
    file("onboarding", PathClass::Config, artifact_file::ONBOARDING),
    file(
        "pane-layouts",
        PathClass::Config,
        artifact_file::PANE_LAYOUTS,
    ),
    file("automations", PathClass::Config, artifact_file::AUTOMATIONS),
    file("repo-trust", PathClass::Config, artifact_file::REPO_TRUST),
    file(
        "default-tabs-applied",
        PathClass::Config,
        artifact_file::DEFAULT_TABS_APPLIED,
    ),
    file(
        "quick-commands",
        PathClass::Config,
        artifact_file::QUICK_COMMANDS,
    ),
    file("diff-notes", PathClass::Config, artifact_file::DIFF_NOTES),
    file(
        "stage-layouts",
        PathClass::Config,
        artifact_file::STAGE_LAYOUTS,
    ),
    file(
        "hook-preference",
        PathClass::Config,
        crate::hooks::SETTINGS_FILE_NAME,
    ),
    ArtifactSpec {
        key: "account-repositories",
        class: PathClass::Config,
        layout: ArtifactLayout::AccountRepositories,
        migration: MigrationKind::Transform,
    },
    ArtifactSpec {
        key: "jira-repository",
        class: PathClass::Config,
        layout: ArtifactLayout::JiraRepository,
        migration: MigrationKind::Transform,
    },
    file(
        "automation-runs",
        PathClass::LocalData,
        artifact_file::AUTOMATION_RUNS,
    ),
    file(
        "claude-usage",
        PathClass::LocalData,
        artifact_file::CLAUDE_USAGE,
    ),
    file(
        "codex-usage",
        PathClass::LocalData,
        artifact_file::CODEX_USAGE,
    ),
    file(
        "grok-usage",
        PathClass::LocalData,
        artifact_file::GROK_USAGE,
    ),
    file(
        "opencode-usage",
        PathClass::LocalData,
        artifact_file::OPENCODE_USAGE,
    ),
    file(
        "kimi-usage",
        PathClass::LocalData,
        artifact_file::KIMI_USAGE,
    ),
    file(
        "popout-bounds",
        PathClass::LocalData,
        artifact_file::POPOUT_BOUNDS,
    ),
    file(
        "main-window-bounds",
        PathClass::LocalData,
        artifact_file::MAIN_WINDOW_BOUNDS,
    ),
    file(
        "webview-errors",
        PathClass::LocalData,
        artifact_file::WEBVIEW_ERRORS,
    ),
    ArtifactSpec {
        key: "serve-tokens",
        class: PathClass::LocalData,
        layout: ArtifactLayout::Prefix(artifact_file::SERVE_TOKEN_PREFIX),
        migration: MigrationKind::Copy,
    },
    directory(
        "hook-endpoint",
        PathClass::LocalData,
        crate::hooks::ENDPOINT_DIR_NAME,
        MigrationKind::Recreate,
    ),
    directory(
        "agent-team-shim",
        PathClass::LocalData,
        crate::agent_teams::SHIM_DIR_NAME,
        MigrationKind::Recreate,
    ),
    file(
        "codex-hook-backup",
        PathClass::LocalData,
        artifact_file::CODEX_HOOK_BACKUP,
    ),
    directory(
        artifact_file::CODEX_RUNTIME_HOME,
        PathClass::LocalData,
        artifact_file::CODEX_RUNTIME_HOME,
        MigrationKind::Recreate,
    ),
    directory(
        crate::icon::CACHE_DIR_NAME,
        PathClass::Cache,
        "agent-icons",
        MigrationKind::Recreate,
    ),
    directory(
        crate::jira_attachments::DIR_NAME,
        PathClass::LocalData,
        "jira-attachments",
        MigrationKind::Recreate,
    ),
];

const fn file(key: &'static str, class: PathClass, relative: &'static str) -> ArtifactSpec {
    ArtifactSpec {
        key,
        class,
        layout: ArtifactLayout::File(relative),
        migration: MigrationKind::Copy,
    }
}

const fn directory(
    key: &'static str,
    class: PathClass,
    relative: &'static str,
    migration: MigrationKind,
) -> ArtifactSpec {
    ArtifactSpec {
        key,
        class,
        layout: ArtifactLayout::Directory(relative),
        migration,
    }
}

/// Canonical, ordered identity of the complete migration inventory.
///
/// Receipts persist these descriptors rather than only human-facing keys, so
/// moving an artifact to another class, changing its shape, or renaming one
/// of a repository's owned files invalidates the old migration contract.
pub(crate) fn artifact_manifest_entries() -> Vec<String> {
    ARTIFACTS.iter().map(artifact_manifest_entry).collect()
}

fn artifact_manifest_entry(artifact: &ArtifactSpec) -> String {
    format!(
        "{}|{}|{}|{}",
        artifact.key,
        path_class_name(artifact.class),
        layout_manifest(artifact.layout),
        migration_kind_name(artifact.migration),
    )
}

const fn path_class_name(class: PathClass) -> &'static str {
    match class {
        PathClass::Config => "config",
        PathClass::LocalData => "local-data",
        PathClass::Cache => "cache",
    }
}

fn layout_manifest(layout: ArtifactLayout) -> String {
    match layout {
        ArtifactLayout::File(path) => format!("file:{path}"),
        ArtifactLayout::Directory(path) => format!("directory:{path}"),
        ArtifactLayout::Prefix(path) => format!("prefix:{path}"),
        ArtifactLayout::SettingsRepository => format!(
            "settings:schema={};canonical=[{}];legacy=[{}];recovery=[{}]",
            crate::settings::PATH_MIGRATION_SCHEMA,
            SETTINGS_DOCUMENTS.join(","),
            legacy_settings_file::ALL.join(","),
            crate::settings::PATH_MIGRATION_RECOVERY_MARKERS.join(","),
        ),
        ArtifactLayout::JiraRepository => format!(
            "jira:schema={};metadata={};legacy-tokens={};transactions=[{}]",
            crate::jira_store::PATH_MIGRATION_SCHEMA,
            crate::jira_store::SITE_FILE_NAME,
            crate::jira_store::TOKEN_DIRECTORY_NAME,
            crate::jira_store::PATH_MIGRATION_TRANSACTION_PARTS.join(","),
        ),
        ArtifactLayout::AccountRepositories => format!(
            "accounts:schema={};claude-index={};claude-homes={};claude-path-field={};codex-index={};codex-homes={};codex-path-field={}",
            ACCOUNT_REPOSITORIES_MIGRATION_SCHEMA,
            crate::accounts::ACCOUNT_STORE_FILE,
            crate::accounts::MANAGED_ACCOUNTS_DIR,
            CLAUDE_ACCOUNT_PATH_FIELD,
            crate::codex_accounts::ACCOUNT_STORE_FILE,
            crate::codex_accounts::MANAGED_ACCOUNTS_DIR,
            CODEX_ACCOUNT_PATH_FIELD,
        ),
    }
}

const fn migration_kind_name(kind: MigrationKind) -> &'static str {
    match kind {
        MigrationKind::Copy => "copy",
        MigrationKind::Transform => "transform",
        MigrationKind::Recreate => "recreate",
    }
}

/// Resolve targets through Tauri once the application context exists.
pub(crate) fn resolve<R: tauri::Runtime>(app: &tauri::App<R>) -> Result<AppPaths, String> {
    resolve_with(|| resolve_targets(app), legacy_state_root)
}

type TargetPaths = (PathBuf, PathBuf, PathBuf);

fn resolve_targets<R: tauri::Runtime>(app: &tauri::App<R>) -> Result<TargetPaths, String> {
    Ok((
        app.path()
            .app_config_dir()
            .map_err(|error| error.to_string())?,
        app.path()
            .app_local_data_dir()
            .map_err(|error| error.to_string())?,
        app.path()
            .app_cache_dir()
            .map_err(|error| error.to_string())?,
    ))
}

fn resolve_with(
    resolve_targets: impl FnOnce() -> Result<TargetPaths, String>,
    discover_legacy: impl FnOnce() -> Option<PathBuf>,
) -> Result<AppPaths, String> {
    let legacy = discover_legacy();
    let (config, local_data, cache) = resolve_targets()?;
    Ok(AppPaths::from_resolved(config, local_data, cache, legacy))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn the_bundle_identifier_has_one_spelling() {
        let config: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri config JSON");
        assert_eq!(
            config.get("identifier").and_then(serde_json::Value::as_str),
            Some(zerocode_lane::APP_IDENTIFIER)
        );
    }

    #[test]
    fn every_manifest_key_and_concrete_path_is_unique() {
        let mut keys = BTreeSet::new();
        let mut paths = BTreeSet::new();
        for artifact in ARTIFACTS {
            assert!(keys.insert(artifact.key), "duplicate key: {}", artifact.key);
            let relative = match artifact.layout {
                ArtifactLayout::File(path)
                | ArtifactLayout::Directory(path)
                | ArtifactLayout::Prefix(path) => Some(path),
                ArtifactLayout::SettingsRepository
                | ArtifactLayout::JiraRepository
                | ArtifactLayout::AccountRepositories => None,
            };
            if let Some(relative) = relative {
                assert!(
                    paths.insert(relative),
                    "two migration units own the same path: {relative}"
                );
            }
        }
    }

    #[test]
    fn receipt_manifest_identity_tracks_the_exact_ordered_inventory() {
        let inventory = artifact_manifest_entries();
        assert_eq!(
            zerocode_lane::artifact_manifest_id(&inventory),
            zerocode_lane::PATH_MIGRATION_MANIFEST_ID,
            "update the shared receipt contract whenever the migration inventory changes"
        );
    }

    #[test]
    fn account_indexes_and_managed_homes_are_one_transform() {
        let account_artifacts = ARTIFACTS
            .iter()
            .filter(|artifact| {
                artifact.key.contains("account")
                    || matches!(artifact.layout, ArtifactLayout::AccountRepositories)
            })
            .collect::<Vec<_>>();
        assert_eq!(account_artifacts.len(), 1);
        assert_eq!(account_artifacts[0].key, "account-repositories");
        assert_eq!(account_artifacts[0].migration, MigrationKind::Transform);
        assert_eq!(
            account_artifacts[0].layout,
            ArtifactLayout::AccountRepositories
        );
    }

    #[test]
    fn legacy_discovery_is_independent_from_target_resolution() {
        let legacy_was_discovered = Cell::new(false);

        let error = resolve_with(
            || Err("target resolution failed".to_string()),
            || {
                legacy_was_discovered.set(true);
                Some(PathBuf::from("legacy-state"))
            },
        )
        .expect_err("target failure must still fail the complete path contract");

        assert_eq!(error, "target resolution failed");
        assert!(legacy_was_discovered.get());
    }

    #[test]
    fn resolved_targets_retain_the_independently_discovered_legacy_root() {
        let legacy = PathBuf::from("legacy-state");
        let paths = resolve_with(
            || {
                Ok((
                    PathBuf::from("config"),
                    PathBuf::from("local-data"),
                    PathBuf::from("cache"),
                ))
            },
            || Some(legacy.clone()),
        )
        .expect("independent target and legacy sources should compose");

        assert_eq!(paths.legacy_state(), Some(legacy.as_path()));
    }

    /// This is the activation gate: runtime paths come only from AppState or
    /// an explicitly injected classified root. Test-source string fixtures do
    /// not count as callers.
    #[test]
    fn platform_paths_activate_only_after_all_legacy_callers_are_removed() {
        let sources = [
            include_str!("main.rs"),
            include_str!("cmd/mod.rs"),
            include_str!(concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../zerocode-shell-cmd-jira/src/lib.rs"
            )),
            include_str!("cmd/remote.rs"),
            include_str!("cmd/browser.rs"),
            include_str!("cmd/usage.rs"),
            include_str!("cmd/appearance.rs"),
            include_str!("cmd/fs.rs"),
            include_str!("cmd/onboarding.rs"),
            include_str!("cmd/agent_launch.rs"),
            include_str!("cmd/board.rs"),
            include_str!("cmd/review.rs"),
            include_str!("cmd/repo_policy.rs"),
            include_str!("cmd/integration_prefs.rs"),
            include_str!("cmd/workspace.rs"),
            include_str!("cmd/project.rs"),
            include_str!("cmd/terminal.rs"),
            include_str!("cmd/scm.rs"),
            include_str!("cmd/worktree.rs"),
            include_str!("cmd/settings.rs"),
            include_str!("cmd/session.rs"),
            include_str!("accounts.rs"),
            include_str!("codex_accounts.rs"),
            include_str!("hooks.rs"),
            include_str!("agent_teams.rs"),
            include_str!("icon.rs"),
            include_str!("../../zerocode-app/src/main.rs"),
        ];
        let legacy_callers = sources
            .iter()
            .map(|source| {
                source
                    .split("\n#[cfg(test)]\n")
                    .next()
                    .unwrap_or(source)
                    .matches("default_state_dir()")
                    .count()
            })
            .sum::<usize>();
        assert_eq!(
            legacy_callers, 0,
            "{legacy_callers} legacy path callers remain; changing one root would split state"
        );
        let main = sources[0]
            .split("\n#[cfg(test)]\n")
            .next()
            .unwrap_or(sources[0]);
        assert!(!main.contains("app_paths::active_root"));
        assert!(!main.contains("app_paths::install"));
    }
}
