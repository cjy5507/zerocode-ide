//! Startup workspace risk classification.
//!
//! This module is intentionally pure and UI-free: it decides whether a cwd is
//! too broad to silently inherit the default full-access permission posture. A
//! later interactive trust prompt can reuse this classifier without entangling
//! prompting with permission-mode parsing.

use std::path::{Path, PathBuf};

use core_types::paths::{default_config_home, zo_global_config_roots, ZO_DIR_NAME};
use runtime::PermissionMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WorkspaceRisk {
    Normal,
    HighBlastRadius,
}

impl WorkspaceRisk {
    #[must_use]
    pub(crate) const fn requires_safe_default(self) -> bool {
        matches!(self, Self::HighBlastRadius)
    }
}

#[must_use]
pub(crate) fn classify_cwd(cwd: &Path) -> WorkspaceRisk {
    let cwd = normalize_existing_path(cwd);
    if is_filesystem_root(&cwd) || is_global_home_or_ancestor(&cwd) {
        WorkspaceRisk::HighBlastRadius
    } else {
        WorkspaceRisk::Normal
    }
}

fn is_filesystem_root(path: &Path) -> bool {
    path.parent().is_none()
}

fn is_global_home_or_ancestor(cwd: &Path) -> bool {
    global_home_roots()
        .into_iter()
        .any(|root| root.matches_cwd(cwd))
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct GlobalHomeRoot {
    path: PathBuf,
    protect_ancestors: bool,
}

impl GlobalHomeRoot {
    fn matches_cwd(&self, cwd: &Path) -> bool {
        paths_equal(cwd, &self.path)
            || (self.protect_ancestors && path_starts_with(&self.path, cwd))
    }
}

#[cfg(not(windows))]
fn paths_equal(left: &Path, right: &Path) -> bool {
    left == right
}

#[cfg(windows)]
fn paths_equal(left: &Path, right: &Path) -> bool {
    windows_path_components(left) == windows_path_components(right)
}

#[cfg(not(windows))]
fn path_starts_with(path: &Path, base: &Path) -> bool {
    path.starts_with(base)
}

#[cfg(windows)]
fn path_starts_with(path: &Path, base: &Path) -> bool {
    let path = windows_path_components(path);
    let base = windows_path_components(base);
    path.len() >= base.len() && path[..base.len()] == base
}

#[cfg(windows)]
fn windows_path_components(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

fn global_home_roots() -> Vec<GlobalHomeRoot> {
    let mut roots = Vec::new();
    for config_home in zo_global_config_roots()
        .into_iter()
        .filter(|path| !path.as_os_str().is_empty())
    {
        let config_home = normalize_existing_path(&config_home);
        push_global_home_root(
            &mut roots,
            GlobalHomeRoot {
                path: config_home.clone(),
                protect_ancestors: false,
            },
        );
        if is_conventional_zo_config_dir(&config_home) {
            if let Some(parent) = config_home.parent() {
                push_global_home_root(
                    &mut roots,
                    GlobalHomeRoot {
                        path: parent.to_path_buf(),
                        protect_ancestors: true,
                    },
                );
            }
        }
    }
    roots
}

fn push_global_home_root(roots: &mut Vec<GlobalHomeRoot>, root: GlobalHomeRoot) {
    if let Some(existing) = roots.iter_mut().find(|existing| existing.path == root.path) {
        existing.protect_ancestors |= root.protect_ancestors;
    } else {
        roots.push(root);
    }
}

fn is_conventional_zo_config_dir(path: &Path) -> bool {
    path.file_name()
        .is_some_and(|name| name == std::ffi::OsStr::new(ZO_DIR_NAME))
}

fn normalize_existing_path(path: &Path) -> PathBuf {
    if let Ok(canonical) = std::fs::canonicalize(path) {
        return canonical;
    }
    if let (Some(parent), Some(file_name)) = (path.parent(), path.file_name()) {
        if !parent.as_os_str().is_empty() && parent != path {
            return normalize_existing_path(parent).join(file_name);
        }
    }
    core_types::paths::normalize_path_components(path)
}

// ============================================================================
// Interactive trust gate (Claude-Code-style "do you trust this folder?")
// ============================================================================

/// Persisted decisions, keyed by canonical cwd → permission-mode label, under
/// `~/.zo/trusted_workspaces.json`. Remembered so a folder is asked about
/// only on its first interactive visit.
const TRUST_STORE_FILE: &str = "trusted_workspaces.json";

/// The single canonical *write* location for trust decisions: the primary
/// (highest-priority) global config home. Reads still consult every root; only
/// writes land here, so a merged view never gets scattered back across roots.
fn primary_trust_store_path() -> PathBuf {
    default_config_home().join(TRUST_STORE_FILE)
}

/// Merge the trust store across every canonical global config root, with
/// higher-priority roots winning on key collision.
///
/// `zo_global_config_roots()` is highest-priority first (`ZO_CONFIG_HOME` →
/// `ZO_HOME` → `~/.zo` → read-only legacy `~/.forge`). We apply the roots
/// low-to-high so a higher-priority root's decision overwrites a lower one's
/// for the same workspace key, giving deterministic precedence regardless of
/// how many roots exist.
fn load_trust_store() -> std::collections::BTreeMap<String, String> {
    let mut roots = zo_global_config_roots();
    if roots.is_empty() {
        roots.push(default_config_home());
    }
    let mut merged = std::collections::BTreeMap::new();
    for root in roots.into_iter().rev() {
        let path = root.join(TRUST_STORE_FILE);
        // A trust decision is authority, not ordinary configuration. Refuse a
        // file unless the same owner-only property is proven as Unix 0600:
        // current owner, one link, no symlink/junction component, and either
        // zero group/other bits or a protected single-SID Windows DACL.
        if !runtime::secure_fs::is_owned_private_regular_file_absolute(&path)
            .unwrap_or(false)
        {
            continue;
        }
        let Some(entries) = runtime::secure_fs::read_regular_file_absolute_no_follow(&path)
            .ok()
            .flatten()
            .and_then(|raw| {
                serde_json::from_str::<std::collections::BTreeMap<String, String>>(&raw).ok()
            })
        else {
            continue;
        };
        merged.extend(entries);
    }
    merged
}

/// Persist the merged trust store to the primary root only, hardening the
/// directory and file to owner-only access (`0o700`/`0o600` on Unix) so a
/// no-home fallback home or a fresh config dir never leaves trust decisions
/// world-readable.
#[cfg(test)]
fn save_trust_store(store: &std::collections::BTreeMap<String, String>) {
    let _ = try_save_trust_store(store);
}

fn try_save_trust_store(
    store: &std::collections::BTreeMap<String, String>,
) -> std::io::Result<()> {
    let path = primary_trust_store_path();
    let json = serde_json::to_string_pretty(store).map_err(std::io::Error::other)?;
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "workspace trust store has no parent directory",
        )
    })?;
    runtime::secure_fs::ensure_private_dir_absolute(parent, 1)?;
    replace_private_trust_store(&path, json.as_bytes())
}

fn replace_private_trust_store(path: &Path, contents: &[u8]) -> std::io::Result<()> {
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "workspace trust store has no parent directory",
        )
    })?;
    let file_name = path.file_name().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "workspace trust store has no file name",
        )
    })?;
    runtime::secure_fs::write_atomic_owner_only(parent, Path::new(file_name), contents)
}

use crate::permission_mode::{mode_from_label, mode_label};

fn trust_key(cwd: &Path) -> String {
    let normalized = normalize_existing_path(cwd);
    #[cfg(windows)]
    {
        // `canonicalize` returns `\\?\` paths on Windows. Persist one
        // separator/case-insensitive spelling so the same directory does not
        // lose its decision when entered through a drive-letter spelling.
        let rendered = normalized.to_string_lossy().replace('/', "\\");
        rendered
            .strip_prefix(r"\\?\")
            .unwrap_or(&rendered)
            .to_lowercase()
    }
    #[cfg(not(windows))]
    normalized.to_string_lossy().into_owned()
}

/// Resolve the permission mode for the *interactive* (TUI) entry, applying the
/// CC-style trust gate.
///
/// - A folder already recorded in the trust store reuses its saved mode — no
///   prompt, so a trusted project opens straight into its chosen mode.
/// - A first interactive visit (a real TTY on both stdin and stdout) shows the
///   3-way prompt, persists the choice, and uses it.
/// - With no TTY (headless redirect, CI, a piped stdin) there is nothing to
///   prompt, so the caller's risk-aware `default_for` is returned unchanged —
///   this is why `-p`/serve never block on a prompt.
pub(crate) fn resolve_trust_for_cwd(
    cwd: &Path,
    default_for: impl Fn(&Path) -> PermissionMode,
    inline: bool,
) -> PermissionMode {
    // The settings override retires the gate outright: the user asked for
    // every folder to open in full access with no prompt. It outranks the
    // store so a folder once remembered as read-only cannot resurrect the
    // prompt or its old answer.
    if crate::preferences::load().grants_full_trust_everywhere() {
        return PermissionMode::DangerFullAccess;
    }
    let key = trust_key(cwd);
    let mut store = load_trust_store();
    if let Some(saved) = store.get(&key).and_then(|label| mode_from_label(label)) {
        return saved;
    }
    if is_interactive_tty() {
        if let Some(chosen) = prompt_workspace_trust(cwd, inline) {
            store.insert(key, mode_label(chosen).to_string());
            if let Err(error) = try_save_trust_store(&store) {
                eprintln!(
                    "[zo] warning: failed to remember workspace trust for {}: {error}",
                    cwd.display()
                );
            }
            return chosen;
        }
    }
    default_for(cwd)
}

fn is_interactive_tty() -> bool {
    use std::io::IsTerminal;
    std::io::stdin().is_terminal() && std::io::stdout().is_terminal()
}

/// Render the 3-way trust prompt and read a choice from stdin. `None` only if
/// the read fails (e.g. EOF), in which case the caller keeps its safe default.
/// Plain 3-way trust prompt on a cooked-mode terminal — numbered list on
/// stdout, one line read from stdin. `None` on EOF/unreadable input so the
/// caller keeps its safe default. (Ported from the ratatui modal; the pane
/// never enters raw mode, so the modal itself did not come along.)
fn prompt_workspace_trust(cwd: &Path, _inline: bool) -> Option<PermissionMode> {
    use std::io::Write;
    let choices = [
        (PermissionMode::DangerFullAccess, "full access (trust this folder)"),
        (PermissionMode::WorkspaceWrite, "workspace-write (edits inside the folder, ask elsewhere)"),
        (PermissionMode::ReadOnly, "read-only"),
    ];
    let mut out = std::io::stdout();
    let _ = writeln!(out, "Trust this folder? {}", cwd.display());
    for (index, (_, label)) in choices.iter().enumerate() {
        let _ = writeln!(out, "  {}. {label}", index + 1);
    }
    let _ = write!(out, "choice [1-3, Enter = 2]: ");
    let _ = out.flush();
    let mut line = String::new();
    if std::io::stdin().read_line(&mut line).ok()? == 0 {
        return None;
    }
    let picked = match line.trim() {
        "" | "2" => 1,
        "1" => 0,
        "3" => 2,
        _ => return None,
    };
    Some(choices[picked].0)
}

#[cfg(test)]
mod tests {
    use super::{classify_cwd, WorkspaceRisk};
    use std::path::Path;
    


    fn with_home<T>(home: &Path, f: impl FnOnce() -> T) -> T {
        with_env_paths(&[("HOME", Some(home))], f)
    }

    fn with_env_paths<T>(vars: &[(&str, Option<&Path>)], f: impl FnOnce() -> T) -> T {
        let previous = vars
            .iter()
            .map(|(key, _)| (*key, std::env::var_os(key)))
            .collect::<Vec<_>>();
        for (key, value) in vars {
            match value {
                Some(path) => std::env::set_var(key, path),
                None => std::env::remove_var(key),
            }
        }
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
        for (key, value) in previous {
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
        }
        match result {
            Ok(value) => value,
            Err(payload) => std::panic::resume_unwind(payload),
        }
    }

    #[test]
    fn workspace_trust_marks_home_as_high_blast_radius() {
        let _guard = crate::test_env_lock();
        let root = crate::support::temp_dir("home");
        let home = root.join("joe");
        std::fs::create_dir_all(&home).expect("home dir");
        let risk = with_home(&home, || classify_cwd(&home));
        std::fs::remove_dir_all(root).expect("cleanup");
        assert_eq!(risk, WorkspaceRisk::HighBlastRadius);
    }

    #[test]
    fn workspace_trust_marks_home_ancestor_as_high_blast_radius() {
        let _guard = crate::test_env_lock();
        let root = crate::support::temp_dir("ancestor");
        let home = root.join("users").join("joe");
        std::fs::create_dir_all(&home).expect("home dir");
        let users_dir = home.parent().expect("home parent").to_path_buf();
        let risk = with_home(&home, || classify_cwd(&users_dir));
        std::fs::remove_dir_all(root).expect("cleanup");
        assert_eq!(risk, WorkspaceRisk::HighBlastRadius);
    }

    #[test]
    fn workspace_trust_allows_project_below_home() {
        let _guard = crate::test_env_lock();
        let root = crate::support::temp_dir("project");
        let home = root.join("joe");
        let project = home.join("repo");
        std::fs::create_dir_all(&project).expect("project dir");
        let risk = with_home(&home, || classify_cwd(&project));
        std::fs::remove_dir_all(root).expect("cleanup");
        assert_eq!(risk, WorkspaceRisk::Normal);
    }

    #[test]
    fn workspace_trust_marks_zo_config_home_parent_as_high_blast_radius() {
        let _guard = crate::test_env_lock();
        let root = crate::support::temp_dir("zo-config-home");
        let protected_home = root.join("global-home");
        let config_home = protected_home.join(".zo");
        let unrelated_home = root.join("unrelated-home");
        std::fs::create_dir_all(&config_home).expect("config home dir");
        std::fs::create_dir_all(&unrelated_home).expect("unrelated home dir");
        let risk = with_env_paths(
            &[
                ("HOME", Some(&unrelated_home)),
                ("ZO_CONFIG_HOME", Some(&config_home)),
                ("ZO_HOME", None),
            ],
            || classify_cwd(&protected_home),
        );
        std::fs::remove_dir_all(root).expect("cleanup");
        assert_eq!(risk, WorkspaceRisk::HighBlastRadius);
    }

    #[test]
    fn workspace_trust_does_not_mark_arbitrary_config_parent_as_high_blast_radius() {
        let _guard = crate::test_env_lock();
        let root = crate::support::temp_dir("custom-config-home");
        let config_parent = root.join("config-parent");
        let config_home = config_parent.join("zo-config");
        let unrelated_home = root.join("unrelated-home");
        std::fs::create_dir_all(&config_home).expect("config home dir");
        std::fs::create_dir_all(&unrelated_home).expect("unrelated home dir");
        let risk = with_env_paths(
            &[
                ("HOME", Some(&unrelated_home)),
                ("ZO_CONFIG_HOME", Some(&config_home)),
                ("ZO_HOME", None),
            ],
            || classify_cwd(&config_parent),
        );
        std::fs::remove_dir_all(root).expect("cleanup");
        assert_eq!(risk, WorkspaceRisk::Normal);
    }

    #[test]
    fn workspace_trust_marks_filesystem_root_as_high_blast_radius() {
        let _guard = crate::test_env_lock();
        let mut root = std::env::temp_dir();
        while let Some(parent) = root.parent() {
            root = parent.to_path_buf();
        }
        assert_eq!(classify_cwd(&root), WorkspaceRisk::HighBlastRadius);
    }

    #[test]
    fn mode_label_roundtrips_all_modes() {
        use super::{mode_from_label, mode_label};
        use runtime::PermissionMode;
        for mode in [
            PermissionMode::ReadOnly,
            PermissionMode::WorkspaceWrite,
            PermissionMode::DangerFullAccess,
            PermissionMode::Prompt,
            PermissionMode::Allow,
        ] {
            assert_eq!(mode_from_label(mode_label(mode)), Some(mode));
        }
    }

    #[test]
    fn resolve_trust_reuses_recorded_mode_without_prompt() {
        use super::{resolve_trust_for_cwd, trust_key};
        use runtime::PermissionMode;
        let _guard = crate::test_env_lock();
        let root = crate::support::temp_dir("trust-store");
        let config_home = root.join(".zo");
        let project = root.join("project");
        std::fs::create_dir_all(&config_home).expect("config home dir");
        std::fs::create_dir_all(&project).expect("project dir");
        with_env_paths(
            &[
                ("ZO_CONFIG_HOME", Some(config_home.as_path())),
                ("ZO_HOME", None),
            ],
            || {
                // Pre-seed: this folder was trusted as full access.
                let key = trust_key(&project);
                let store = std::collections::BTreeMap::from([(
                    key,
                    "danger-full-access".to_string(),
                )]);
                core_types::paths::write_private_file(
                    &config_home.join("trusted_workspaces.json"),
                    serde_json::to_string(&store)
                        .expect("serialize store")
                        .as_bytes(),
                    &core_types::paths::ParentDirPolicy::LeaveParent,
                )
                .expect("write private store");
                // The recorded mode wins over the (ReadOnly) default, and the
                // store hit short-circuits before any prompt — safe under test.
                let mode =
                    resolve_trust_for_cwd(&project, |_| PermissionMode::ReadOnly, false);
                assert_eq!(mode, PermissionMode::DangerFullAccess);
            },
        );
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn always_full_setting_beats_the_store_and_the_default() {
        use super::{resolve_trust_for_cwd, trust_key};
        use runtime::PermissionMode;
        let _guard = crate::test_env_lock();
        let root = crate::support::temp_dir("trust-always-full");
        let config_home = root.join(".zo");
        let project = root.join("project");
        std::fs::create_dir_all(&config_home).expect("config home dir");
        std::fs::create_dir_all(&project).expect("project dir");
        with_env_paths(
            &[
                ("ZO_CONFIG_HOME", Some(config_home.as_path())),
                ("ZO_HOME", None),
            ],
            || {
                // The knob the user sets once: no prompt, ever — full access.
                std::fs::write(
                    config_home.join(crate::preferences::PREFERENCES_FILE_NAME),
                    r#"{"workspaceTrust":"always-full"}"#,
                )
                .expect("write settings");
                // Even a folder remembered as read-only opens fully: the
                // override outranks the store, not just the prompt.
                let key = trust_key(&project);
                write_store(&config_home, &[(key.as_str(), "read-only")]);
                let mode =
                    resolve_trust_for_cwd(&project, |_| PermissionMode::ReadOnly, false);
                assert_eq!(mode, PermissionMode::DangerFullAccess);
            },
        );
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    #[test]
    fn unrecognized_workspace_trust_value_keeps_the_gate() {
        use super::resolve_trust_for_cwd;
        use runtime::PermissionMode;
        let _guard = crate::test_env_lock();
        let root = crate::support::temp_dir("trust-typo-value");
        let config_home = root.join(".zo");
        let project = root.join("project");
        std::fs::create_dir_all(&config_home).expect("config home dir");
        std::fs::create_dir_all(&project).expect("project dir");
        with_env_paths(
            &[
                ("ZO_CONFIG_HOME", Some(config_home.as_path())),
                ("ZO_HOME", None),
            ],
            || {
                std::fs::write(
                    config_home.join(crate::preferences::PREFERENCES_FILE_NAME),
                    r#"{"workspaceTrust":"sometimes"}"#,
                )
                .expect("write settings");
                let mode =
                    resolve_trust_for_cwd(&project, |_| PermissionMode::ReadOnly, false);
                assert_eq!(mode, PermissionMode::ReadOnly);
            },
        );
        std::fs::remove_dir_all(root).expect("cleanup");
    }

    fn write_store(dir: &Path, entries: &[(&str, &str)]) {
        let store = entries
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect::<std::collections::BTreeMap<String, String>>();
        core_types::paths::write_private_file(
            &dir.join(super::TRUST_STORE_FILE),
            serde_json::to_string(&store)
                .expect("serialize store")
                .as_bytes(),
            &core_types::paths::ParentDirPolicy::LeaveParent,
        )
        .expect("write private store");
    }

    #[test]
    fn load_trust_store_merges_roots_with_higher_priority_winning() {
        use super::load_trust_store;
        let _guard = crate::test_env_lock();
        let root = crate::support::temp_dir("merge-precedence");
        // Primary (ZO_CONFIG_HOME) is highest priority; secondary (ZO_HOME)
        // lower. Both name the same workspace key with different modes, plus a
        // key unique to each root, so we can prove precedence *and* union.
        let primary = root.join("primary");
        let secondary = root.join("secondary");
        std::fs::create_dir_all(&primary).expect("primary dir");
        std::fs::create_dir_all(&secondary).expect("secondary dir");
        write_store(
            &primary,
            &[("/shared", "danger-full-access"), ("/only-primary", "prompt")],
        );
        write_store(
            &secondary,
            &[("/shared", "read-only"), ("/only-secondary", "workspace-write")],
        );
        let merged = with_env_paths(
            &[
                ("ZO_CONFIG_HOME", Some(primary.as_path())),
                ("ZO_HOME", Some(secondary.as_path())),
                ("HOME", None),
            ],
            load_trust_store,
        );
        std::fs::remove_dir_all(&root).expect("cleanup");
        // Higher-priority primary wins the collision; both unique keys survive.
        assert_eq!(merged.get("/shared").map(String::as_str), Some("danger-full-access"));
        assert_eq!(merged.get("/only-primary").map(String::as_str), Some("prompt"));
        assert_eq!(
            merged.get("/only-secondary").map(String::as_str),
            Some("workspace-write")
        );
    }

    #[test]
    fn save_trust_store_writes_primary_root_only() {
        use super::{load_trust_store, save_trust_store, TRUST_STORE_FILE};
        let _guard = crate::test_env_lock();
        let root = crate::support::temp_dir("primary-only-write");
        let primary = root.join("primary");
        let secondary = root.join("secondary");
        std::fs::create_dir_all(&primary).expect("primary dir");
        std::fs::create_dir_all(&secondary).expect("secondary dir");
        // Seed a decision only in the lower-priority root.
        write_store(&secondary, &[("/from-secondary", "read-only")]);
        with_env_paths(
            &[
                ("ZO_CONFIG_HOME", Some(primary.as_path())),
                ("ZO_HOME", Some(secondary.as_path())),
                ("HOME", None),
            ],
            || {
                // Load merges both roots, then we record a new decision.
                let mut store = load_trust_store();
                store.insert("/new-project".to_string(), "prompt".to_string());
                save_trust_store(&store);
            },
        );
        // The write landed in primary only; the secondary file is untouched.
        assert!(primary.join(TRUST_STORE_FILE).exists());
        let secondary_raw =
            std::fs::read_to_string(secondary.join(TRUST_STORE_FILE)).expect("secondary store");
        let secondary_store: std::collections::BTreeMap<String, String> =
            serde_json::from_str(&secondary_raw).expect("parse secondary");
        assert_eq!(
            secondary_store,
            std::collections::BTreeMap::from([(
                "/from-secondary".to_string(),
                "read-only".to_string()
            )]),
            "lower-priority root must not be rewritten"
        );
        let primary_raw =
            std::fs::read_to_string(primary.join(TRUST_STORE_FILE)).expect("primary store");
        let primary_store: std::collections::BTreeMap<String, String> =
            serde_json::from_str(&primary_raw).expect("parse primary");
        // Primary holds the full merged view (both roots' keys plus the new one).
        assert_eq!(primary_store.get("/from-secondary").map(String::as_str), Some("read-only"));
        assert_eq!(primary_store.get("/new-project").map(String::as_str), Some("prompt"));
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[test]
    fn legacy_forge_trust_loads_but_save_writes_primary_zo_only() {
        use super::{load_trust_store, save_trust_store, TRUST_STORE_FILE};
        let _guard = crate::test_env_lock();
        let root = crate::support::temp_dir("legacy-forge-store");
        let legacy = root.join(".forge");
        let primary = root.join(".zo");
        std::fs::create_dir_all(&legacy).expect("legacy dir");
        write_store(&legacy, &[("/legacy-project", "read-only")]);
        let legacy_before = std::fs::read_to_string(legacy.join(TRUST_STORE_FILE))
            .expect("legacy store before save");

        with_env_paths(
            &[
                ("ZO_CONFIG_HOME", None),
                ("ZO_HOME", None),
                ("HOME", Some(root.as_path())),
            ],
            || {
                let mut store = load_trust_store();
                assert_eq!(
                    store.get("/legacy-project").map(String::as_str),
                    Some("read-only")
                );
                store.insert("/new-project".to_string(), "prompt".to_string());
                save_trust_store(&store);
            },
        );

        assert!(primary.join(TRUST_STORE_FILE).exists());
        assert_eq!(
            std::fs::read_to_string(legacy.join(TRUST_STORE_FILE))
                .expect("legacy store after save"),
            legacy_before,
            "legacy trust store must remain read-only"
        );
        let primary_store: std::collections::BTreeMap<String, String> = serde_json::from_str(
            &std::fs::read_to_string(primary.join(TRUST_STORE_FILE)).expect("primary store"),
        )
        .expect("parse primary store");
        assert_eq!(
            primary_store.get("/legacy-project").map(String::as_str),
            Some("read-only")
        );
        assert_eq!(primary_store.get("/new-project").map(String::as_str), Some("prompt"));
        std::fs::remove_dir_all(&root).expect("cleanup");
    }

    #[cfg(unix)]
    #[test]
    fn save_trust_store_hardens_directory_and_file_owner_only() {
        use super::{save_trust_store, TRUST_STORE_FILE};
        use std::os::unix::fs::PermissionsExt as _;
        let _guard = crate::test_env_lock();
        let root = crate::support::temp_dir("perms");
        // Config home starts world-accessible; a pre-existing loose store file
        // exercises the "harden an already-broad file" path, not just creation.
        let config_home = root.join("global");
        std::fs::create_dir_all(&config_home).expect("config dir");
        std::fs::set_permissions(&config_home, std::fs::Permissions::from_mode(0o755))
            .expect("loosen dir");
        let store_path = config_home.join(TRUST_STORE_FILE);
        std::fs::write(&store_path, "{}").expect("seed store");
        std::fs::set_permissions(&store_path, std::fs::Permissions::from_mode(0o644))
            .expect("loosen file");
        with_env_paths(
            &[
                ("ZO_CONFIG_HOME", Some(config_home.as_path())),
                ("ZO_HOME", None),
                ("HOME", None),
            ],
            || {
                let store = std::collections::BTreeMap::from([(
                    "/project".to_string(),
                    "prompt".to_string(),
                )]);
                save_trust_store(&store);
            },
        );
        let dir_mode =
            std::fs::metadata(&config_home).expect("dir meta").permissions().mode() & 0o777;
        let file_mode =
            std::fs::metadata(&store_path).expect("file meta").permissions().mode() & 0o777;
        std::fs::remove_dir_all(&root).expect("cleanup");
        assert_eq!(dir_mode, 0o700, "config dir must be owner-only");
        assert_eq!(file_mode, 0o600, "trust store file must be owner-only");
    }

    #[cfg(unix)]
    #[test]
    fn trust_store_replace_failure_preserves_previous_bytes() {
        use super::replace_private_trust_store;
        use std::os::unix::fs::PermissionsExt as _;

        let root = crate::support::temp_dir("atomic-failure");
        std::fs::create_dir_all(&root).expect("create trust directory");
        let path = root.join("trusted_workspaces.json");
        let before = br#"{"/old":"prompt"}"#;
        std::fs::write(&path, before).expect("seed trust store");
        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o555))
            .expect("make trust directory read-only");

        let probe = root.join("probe");
        if std::fs::write(&probe, b"probe").is_ok() {
            let _ = std::fs::remove_file(probe);
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755))
                .expect("restore trust directory permissions");
            let _ = std::fs::remove_dir_all(root);
            return;
        }

        let result = replace_private_trust_store(&path, br#"{"/new":"read-only"}"#);

        std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755))
            .expect("restore trust directory permissions");
        let after = std::fs::read(&path).expect("read trust store after failed save");
        std::fs::remove_dir_all(&root).expect("cleanup");
        assert!(result.is_err(), "allocating the sibling temp must fail");
        assert_eq!(after, before, "failed save must preserve prior trust bytes");
    }

    #[cfg(unix)]
    #[test]
    fn save_trust_store_hardens_no_home_fallback() {
        use super::{load_trust_store, save_trust_store, TRUST_STORE_FILE};
        use core_types::paths::default_config_home;
        use std::os::unix::fs::PermissionsExt as _;
        let _guard = crate::test_env_lock();
        // With no home resolvable at all, default_config_home() allocates a
        // private temporary home; the trust store written there must still be
        // owner-only rather than leaking into a shared temp dir world-readable.
        with_env_paths(
            &[
                ("ZO_CONFIG_HOME", None),
                ("ZO_HOME", None),
                ("HOME", None),
            ],
            || {
                let store = std::collections::BTreeMap::from([(
                    "/project".to_string(),
                    "prompt".to_string(),
                )]);
                save_trust_store(&store);
                assert_eq!(
                    load_trust_store(),
                    store,
                    "no-home fallback writes must be visible to canonical reads"
                );
                let home = default_config_home();
                let store_path = home.join(TRUST_STORE_FILE);
                let file_mode = std::fs::metadata(&store_path)
                    .expect("fallback store meta")
                    .permissions()
                    .mode()
                    & 0o777;
                assert_eq!(file_mode, 0o600, "fallback trust store must be owner-only");
            },
        );
    }
}
