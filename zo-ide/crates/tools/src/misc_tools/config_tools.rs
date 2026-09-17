use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

// --- Input/Output structs ---

#[derive(Debug, Deserialize)]
pub struct ConfigInput {
    pub setting: String,
    pub value: Option<ConfigValue>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct EnterPlanModeInput {}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
pub struct ExitPlanModeInput {}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum ConfigValue {
    String(String),
    Bool(bool),
    Number(f64),
}

#[derive(Debug, Serialize)]
pub struct ConfigOutput {
    pub(crate) success: bool,
    pub(crate) operation: Option<String>,
    pub(crate) setting: Option<String>,
    pub(crate) value: Option<Value>,
    #[serde(rename = "previousValue")]
    pub(crate) previous_value: Option<Value>,
    #[serde(rename = "newValue")]
    pub(crate) new_value: Option<Value>,
    pub(crate) error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlanModeState {
    #[serde(rename = "hadLocalOverride")]
    pub(crate) had_local_override: bool,
    #[serde(rename = "previousLocalMode")]
    pub(crate) previous_local_mode: Option<Value>,
}

#[derive(Debug, Serialize)]
#[allow(clippy::struct_excessive_bools)]
pub struct PlanModeOutput {
    pub(crate) success: bool,
    pub(crate) operation: String,
    pub(crate) changed: bool,
    pub(crate) active: bool,
    pub(crate) managed: bool,
    pub(crate) message: String,
    #[serde(rename = "settingsPath")]
    pub(crate) settings_path: String,
    #[serde(rename = "statePath")]
    pub(crate) state_path: String,
    #[serde(rename = "previousLocalMode")]
    pub(crate) previous_local_mode: Option<Value>,
    #[serde(rename = "currentLocalMode")]
    pub(crate) current_local_mode: Option<Value>,
}

// --- Config helpers ---

#[derive(Clone, Copy)]
pub(crate) enum ConfigScope {
    Global,
    Settings,
}

#[derive(Clone, Copy)]
pub(crate) struct ConfigSettingSpec {
    pub(crate) scope: ConfigScope,
    pub(crate) kind: ConfigKind,
    pub(crate) path: &'static [&'static str],
    pub(crate) options: Option<&'static [&'static str]>,
}

#[derive(Clone, Copy)]
pub(crate) enum ConfigKind {
    Boolean,
    String,
}

// --- Execution ---

pub fn execute_config(input: ConfigInput) -> Result<ConfigOutput, String> {
    let setting = input.setting.trim();
    if setting.is_empty() {
        return Err(String::from("setting must not be empty"));
    }
    let Some(spec) = supported_config_setting(setting) else {
        return Ok(ConfigOutput {
            success: false,
            operation: None,
            setting: None,
            value: None,
            previous_value: None,
            new_value: None,
            error: Some(format!("Unknown setting: \"{setting}\"")),
        });
    };

    let path = config_file_for_scope(spec.scope)?;
    let effective_document = match spec.scope {
        ConfigScope::Global => read_global_config_object()?,
        ConfigScope::Settings => read_json_object(&path)?,
    };

    if let Some(value) = input.value {
        let normalized = normalize_config_value(spec, value)?;
        let previous_value = get_nested_value(&effective_document, spec.path).cloned();
        // Global updates intentionally begin with just the primary document.
        // Lower-priority roots remain inherited instead of being copied into it.
        let mut document = read_json_object(&path)?;
        set_nested_value(&mut document, spec.path, normalized.clone());
        write_json_object(&path, &document)?;
        Ok(ConfigOutput {
            success: true,
            operation: Some(String::from("set")),
            setting: Some(setting.to_string()),
            value: Some(normalized.clone()),
            previous_value,
            new_value: Some(normalized),
            error: None,
        })
    } else {
        Ok(ConfigOutput {
            success: true,
            operation: Some(String::from("get")),
            setting: Some(setting.to_string()),
            value: get_nested_value(&effective_document, spec.path).cloned(),
            previous_value: None,
            new_value: None,
            error: None,
        })
    }
}

const PERMISSION_DEFAULT_MODE_PATH: &[&str] = &["permissions", "defaultMode"];

pub fn execute_enter_plan_mode(_input: EnterPlanModeInput) -> Result<PlanModeOutput, String> {
    let settings_path = config_file_for_scope(ConfigScope::Settings)?;
    let state_path = plan_mode_state_file()?;
    let mut document = read_json_object(&settings_path)?;
    let current_local_mode = get_nested_value(&document, PERMISSION_DEFAULT_MODE_PATH).cloned();
    let current_is_plan =
        matches!(current_local_mode.as_ref(), Some(Value::String(value)) if value == "plan");

    if let Some(state) = read_plan_mode_state(&state_path)? {
        if current_is_plan {
            return Ok(PlanModeOutput {
                success: true,
                operation: String::from("enter"),
                changed: false,
                active: true,
                managed: true,
                message: String::from("Plan mode override is already active for this worktree."),
                settings_path: settings_path.display().to_string(),
                state_path: state_path.display().to_string(),
                previous_local_mode: state.previous_local_mode,
                current_local_mode,
            });
        }
        clear_plan_mode_state(&state_path)?;
    }

    if current_is_plan {
        return Ok(PlanModeOutput {
            success: true,
            operation: String::from("enter"),
            changed: false,
            active: true,
            managed: false,
            message: String::from(
                "Worktree-local plan mode is already enabled outside EnterPlanMode; leaving it unchanged.",
            ),
            settings_path: settings_path.display().to_string(),
            state_path: state_path.display().to_string(),
            previous_local_mode: None,
            current_local_mode,
        });
    }

    let state = PlanModeState {
        had_local_override: current_local_mode.is_some(),
        previous_local_mode: current_local_mode,
    };
    write_plan_mode_state(&state_path, &state)?;
    set_nested_value(
        &mut document,
        PERMISSION_DEFAULT_MODE_PATH,
        Value::String(String::from("plan")),
    );
    write_json_object(&settings_path, &document)?;

    Ok(PlanModeOutput {
        success: true,
        operation: String::from("enter"),
        changed: true,
        active: true,
        managed: true,
        message: String::from("Enabled worktree-local plan mode override."),
        settings_path: settings_path.display().to_string(),
        state_path: state_path.display().to_string(),
        previous_local_mode: state.previous_local_mode,
        current_local_mode: get_nested_value(&document, PERMISSION_DEFAULT_MODE_PATH).cloned(),
    })
}

pub fn execute_exit_plan_mode(_input: ExitPlanModeInput) -> Result<PlanModeOutput, String> {
    let settings_path = config_file_for_scope(ConfigScope::Settings)?;
    let state_path = plan_mode_state_file()?;
    let mut document = read_json_object(&settings_path)?;
    let current_local_mode = get_nested_value(&document, PERMISSION_DEFAULT_MODE_PATH).cloned();
    let current_is_plan =
        matches!(current_local_mode.as_ref(), Some(Value::String(value)) if value == "plan");

    let Some(state) = read_plan_mode_state(&state_path)? else {
        return Ok(PlanModeOutput {
            success: true,
            operation: String::from("exit"),
            changed: false,
            active: current_is_plan,
            managed: false,
            message: String::from("No EnterPlanMode override is active for this worktree."),
            settings_path: settings_path.display().to_string(),
            state_path: state_path.display().to_string(),
            previous_local_mode: None,
            current_local_mode,
        });
    };

    if !current_is_plan {
        clear_plan_mode_state(&state_path)?;
        return Ok(PlanModeOutput {
            success: true,
            operation: String::from("exit"),
            changed: false,
            active: false,
            managed: false,
            message: String::from(
                "Cleared stale EnterPlanMode state because plan mode was already changed outside the tool.",
            ),
            settings_path: settings_path.display().to_string(),
            state_path: state_path.display().to_string(),
            previous_local_mode: state.previous_local_mode,
            current_local_mode,
        });
    }

    if state.had_local_override {
        if let Some(previous_local_mode) = state.previous_local_mode.clone() {
            set_nested_value(
                &mut document,
                PERMISSION_DEFAULT_MODE_PATH,
                previous_local_mode,
            );
        } else {
            remove_nested_value(&mut document, PERMISSION_DEFAULT_MODE_PATH);
        }
    } else {
        remove_nested_value(&mut document, PERMISSION_DEFAULT_MODE_PATH);
    }
    write_json_object(&settings_path, &document)?;
    clear_plan_mode_state(&state_path)?;

    Ok(PlanModeOutput {
        success: true,
        operation: String::from("exit"),
        changed: true,
        active: false,
        managed: false,
        message: String::from("Restored the prior worktree-local plan mode setting."),
        settings_path: settings_path.display().to_string(),
        state_path: state_path.display().to_string(),
        previous_local_mode: state.previous_local_mode,
        current_local_mode: get_nested_value(&document, PERMISSION_DEFAULT_MODE_PATH).cloned(),
    })
}

// --- Config setting registry ---

#[allow(clippy::too_many_lines)] // a flat setting table, clearer unsplit
pub(crate) fn supported_config_setting(setting: &str) -> Option<ConfigSettingSpec> {
    Some(match setting {
        "theme" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::String,
            path: &["theme"],
            options: None,
        },
        // Free-form (no `options`): custom styles live in
        // `.zo/output-styles/*.md`, so the valid set is open-ended —
        // `/output-style` validates against the resolvable styles instead.
        // Scope Settings (= `.zo/settings.local.json`): Claude Code parity,
        // the style choice is per-project.
        "outputStyle" => ConfigSettingSpec {
            scope: ConfigScope::Settings,
            kind: ConfigKind::String,
            path: &["outputStyle"],
            options: None,
        },
        "verbose" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::Boolean,
            path: &["verbose"],
            options: None,
        },
        "voiceInputEnabled" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::Boolean,
            path: &["voiceInputEnabled"],
            options: None,
        },
        "advisorModeEnabled" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::Boolean,
            path: &["advisorModeEnabled"],
            options: None,
        },
        "stickerPack" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::String,
            path: &["stickerPack"],
            options: Some(&["zo-classic", "minimal", "debug"]),
        },
        "autoCompactEnabled" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::Boolean,
            path: &["autoCompactEnabled"],
            options: None,
        },
        "autoMemoryEnabled" => ConfigSettingSpec {
            scope: ConfigScope::Settings,
            kind: ConfigKind::Boolean,
            path: &["autoMemoryEnabled"],
            options: None,
        },
        "autoDreamEnabled" => ConfigSettingSpec {
            scope: ConfigScope::Settings,
            kind: ConfigKind::Boolean,
            path: &["autoDreamEnabled"],
            options: None,
        },
        "recallHintEnabled" => ConfigSettingSpec {
            scope: ConfigScope::Settings,
            kind: ConfigKind::Boolean,
            path: &["recallHintEnabled"],
            options: None,
        },
        "fileCheckpointingEnabled" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::Boolean,
            path: &["fileCheckpointingEnabled"],
            options: None,
        },
        "showTurnDuration" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::Boolean,
            path: &["showTurnDuration"],
            options: None,
        },
        "terminalProgressBarEnabled" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::Boolean,
            path: &["terminalProgressBarEnabled"],
            options: None,
        },
        "todoFeatureEnabled" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::Boolean,
            path: &["todoFeatureEnabled"],
            options: None,
        },
        "model" => ConfigSettingSpec {
            scope: ConfigScope::Settings,
            kind: ConfigKind::String,
            path: &["model"],
            options: None,
        },
        "alwaysThinkingEnabled" => ConfigSettingSpec {
            scope: ConfigScope::Settings,
            kind: ConfigKind::Boolean,
            path: &["alwaysThinkingEnabled"],
            options: None,
        },
        "reasoningEffort" => ConfigSettingSpec {
            scope: ConfigScope::Settings,
            kind: ConfigKind::String,
            path: &["reasoningEffort"],
            // P9: `ultra` is now its own static level (formerly just an
            // alias of `ultracode`) and `ultracode` was renamed to `smart`.
            // `ultracode` stays accepted here too since `Effort::from_token`
            // (the actual reader of this persisted value) still parses it
            // as a legacy alias for `smart` — writers should prefer `smart`.
            options: Some(&[
                "off", "low", "medium", "high", "xhigh", "max", "ultra", "smart", "ultracode",
            ]),
        },
        "permissions.defaultMode" => ConfigSettingSpec {
            scope: ConfigScope::Settings,
            kind: ConfigKind::String,
            path: &["permissions", "defaultMode"],
            options: Some(&["default", "plan", "acceptEdits", "dontAsk", "auto"]),
        },
        "language" => ConfigSettingSpec {
            scope: ConfigScope::Settings,
            kind: ConfigKind::String,
            path: &["language"],
            options: None,
        },
        "teammateMode" => ConfigSettingSpec {
            scope: ConfigScope::Global,
            kind: ConfigKind::String,
            path: &["teammateMode"],
            options: Some(&["tmux", "in-process", "auto"]),
        },
        _ => return None,
    })
}

pub(crate) fn normalize_config_value(
    spec: ConfigSettingSpec,
    value: ConfigValue,
) -> Result<Value, String> {
    let normalized = match (spec.kind, value) {
        (ConfigKind::Boolean, ConfigValue::Bool(value)) => Value::Bool(value),
        (ConfigKind::Boolean, ConfigValue::String(value)) => {
            match value.trim().to_ascii_lowercase().as_str() {
                "true" => Value::Bool(true),
                "false" => Value::Bool(false),
                _ => return Err(String::from("setting requires true or false")),
            }
        }
        (ConfigKind::Boolean, ConfigValue::Number(_)) => {
            return Err(String::from("setting requires true or false"));
        }
        (ConfigKind::String, ConfigValue::String(value)) => Value::String(value),
        (ConfigKind::String, ConfigValue::Bool(value)) => Value::String(value.to_string()),
        (ConfigKind::String, ConfigValue::Number(value)) => serde_json::json!(value),
    };

    if let Some(options) = spec.options {
        let Some(as_str) = normalized.as_str() else {
            return Err(String::from("setting requires a string value"));
        };
        if !options.iter().any(|option| option == &as_str) {
            return Err(format!(
                "Invalid value \"{as_str}\". Options: {}",
                options.join(", ")
            ));
        }
    }

    Ok(normalized)
}

pub(crate) fn config_file_for_scope(scope: ConfigScope) -> Result<PathBuf, String> {
    let cwd = std::env::current_dir().map_err(|error| error.to_string())?;
    Ok(match scope {
        ConfigScope::Global => config_home_dir().join("settings.json"),
        ConfigScope::Settings => cwd.join(".zo").join("settings.local.json"),
    })
}

pub(crate) fn config_home_dir() -> PathBuf {
    runtime::default_config_home()
}

fn read_global_config_object() -> Result<serde_json::Map<String, Value>, String> {
    let mut roots = core_types::paths::zo_global_config_roots();
    if roots.is_empty() {
        roots.push(config_home_dir());
    }

    let mut merged = serde_json::Map::new();
    for root in roots.iter().rev() {
        merge_json_objects(&mut merged, read_json_object(&root.join("settings.json"))?);
    }
    Ok(merged)
}

fn merge_json_objects(
    target: &mut serde_json::Map<String, Value>,
    source: serde_json::Map<String, Value>,
) {
    for (key, value) in source {
        match value {
            Value::Object(source_object) => match target.get_mut(&key) {
                Some(Value::Object(target_object)) => {
                    merge_json_objects(target_object, source_object);
                }
                _ => {
                    target.insert(key, Value::Object(source_object));
                }
            },
            value => {
                target.insert(key, value);
            }
        }
    }
}

pub(crate) fn read_json_object(path: &Path) -> Result<serde_json::Map<String, Value>, String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            if contents.trim().is_empty() {
                return Ok(serde_json::Map::new());
            }
            serde_json::from_str::<Value>(&contents)
                .map_err(|error| error.to_string())?
                .as_object()
                .cloned()
                .ok_or_else(|| String::from("config file must contain a JSON object"))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(serde_json::Map::new()),
        Err(error) => Err(error.to_string()),
    }
}

pub(crate) fn write_json_object(
    path: &Path,
    value: &serde_json::Map<String, Value>,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

pub(crate) fn get_nested_value<'a>(
    value: &'a serde_json::Map<String, Value>,
    path: &[&str],
) -> Option<&'a Value> {
    let (first, rest) = path.split_first()?;
    let mut current = value.get(*first)?;
    for key in rest {
        current = current.as_object()?.get(*key)?;
    }
    Some(current)
}

pub(crate) fn set_nested_value(
    root: &mut serde_json::Map<String, Value>,
    path: &[&str],
    new_value: Value,
) {
    let (first, rest) = path.split_first().expect("config path must not be empty");
    if rest.is_empty() {
        root.insert((*first).to_string(), new_value);
        return;
    }

    let entry = root
        .entry((*first).to_string())
        .or_insert_with(|| Value::Object(serde_json::Map::new()));
    if !entry.is_object() {
        *entry = Value::Object(serde_json::Map::new());
    }
    let map = entry.as_object_mut().expect("object inserted");
    set_nested_value(map, rest, new_value);
}

pub(crate) fn remove_nested_value(
    root: &mut serde_json::Map<String, Value>,
    path: &[&str],
) -> bool {
    let Some((first, rest)) = path.split_first() else {
        return false;
    };
    if rest.is_empty() {
        return root.remove(*first).is_some();
    }

    let mut should_remove_parent = false;
    let removed = root.get_mut(*first).is_some_and(|entry| {
        entry.as_object_mut().is_some_and(|map| {
            let removed = remove_nested_value(map, rest);
            should_remove_parent = removed && map.is_empty();
            removed
        })
    });

    if should_remove_parent {
        root.remove(*first);
    }

    removed
}

pub(crate) fn plan_mode_state_file() -> Result<PathBuf, String> {
    Ok(config_file_for_scope(ConfigScope::Settings)?
        .parent()
        .ok_or_else(|| String::from("settings.local.json has no parent directory"))?
        .join("tool-state")
        .join("plan-mode.json"))
}

pub(crate) fn read_plan_mode_state(path: &Path) -> Result<Option<PlanModeState>, String> {
    match std::fs::read_to_string(path) {
        Ok(contents) => {
            if contents.trim().is_empty() {
                return Ok(None);
            }
            serde_json::from_str(&contents)
                .map(Some)
                .map_err(|error| error.to_string())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

fn write_plan_mode_state(path: &Path, state: &PlanModeState) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(
        path,
        serde_json::to_string_pretty(state).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn clear_plan_mode_state(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
    }
}

/// Markdown body for a persisted plan artifact: a title (the optional summary,
/// else `"Plan"`) followed by the plan text. Pure so it is unit-testable
/// without touching the filesystem.
pub(crate) fn format_plan_markdown(plan: &str, summary: Option<&str>) -> String {
    let title = summary
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("Plan");
    format!("# {title}\n\n{}\n", plan.trim_end())
}

/// Stable, content-addressed id for a plan artifact filename. Hashing the plan
/// (and summary) means resubmitting the same plan overwrites the same file
/// (idempotent) while distinct plans never collide. Not security-sensitive — a
/// filename only — so the standard hasher is fine.
pub(crate) fn plan_artifact_id(plan: &str, summary: Option<&str>) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    plan.hash(&mut hasher);
    summary.hash(&mut hasher);
    format!("plan-{:016x}", hasher.finish())
}

/// How many `.zo/plans/plan-*.md` artifacts to retain. Each submitted plan
/// persists one file (content-addressed, so resubmits overwrite); without a
/// bound the directory grows without limit since nothing ever deleted them.
/// The most recent runs are the only ones a human reviews, so keep a generous
/// rolling window and prune the rest.
const PLAN_ARTIFACT_RETENTION: usize = 20;

/// Persist a submitted plan as an editable artifact under `.zo/plans/`, so a
/// human can review (and edit) it before approving — the plan-approval gate
/// never lets the model self-approve. Returns the written path. Mirrors
/// [`write_plan_mode_state`]'s create-dir-then-write idiom.
pub(crate) fn write_plan_artifact(plan: &str, summary: Option<&str>) -> Result<PathBuf, String> {
    let dir = config_file_for_scope(ConfigScope::Settings)?
        .parent()
        .ok_or_else(|| String::from("settings.local.json has no parent directory"))?
        .join("plans");
    std::fs::create_dir_all(&dir).map_err(|error| error.to_string())?;
    let path = dir.join(format!("{}.md", plan_artifact_id(plan, summary)));
    std::fs::write(&path, format_plan_markdown(plan, summary))
        .map_err(|error| error.to_string())?;
    // Bound the directory: keep the newest `PLAN_ARTIFACT_RETENTION` artifacts
    // and drop older ones. Best-effort — a prune failure never fails the write.
    prune_plan_artifacts(&dir, PLAN_ARTIFACT_RETENTION, Some(&path));
    Ok(path)
}

/// Delete the oldest `plan-*.md` artifacts in `dir` so at most `keep` remain,
/// ranked newest-first by modification time. `always_keep` (the just-written
/// file) is retained unconditionally and excluded from the ranking, so a clock
/// tie can never prune the artifact this call just created. Only files matching
/// the `plan-*.md` naming are touched — sibling files are left alone — and
/// every filesystem error is swallowed: pruning is housekeeping, not the
/// caller's contract.
/// Whether `path` is a persisted plan artifact (`plan-*.md`). Extension is
/// matched via [`Path::extension`] (not a case-sensitive `ends_with`) so the
/// check is robust and lint-clean.
fn is_plan_artifact(path: &Path) -> bool {
    let is_md = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("md"));
    let named_plan = path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("plan-"));
    is_md && named_plan
}

fn prune_plan_artifacts(dir: &Path, keep: usize, always_keep: Option<&Path>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut artifacts: Vec<(std::time::SystemTime, PathBuf)> = entries
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if always_keep.is_some_and(|kept| kept == path) {
                return None;
            }
            if !is_plan_artifact(&path) {
                return None;
            }
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, path))
        })
        .collect();
    // `always_keep` already occupies one retention slot when present.
    let budget = keep.saturating_sub(usize::from(always_keep.is_some()));
    if artifacts.len() <= budget {
        return;
    }
    artifacts.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in artifacts.into_iter().skip(budget) {
        let _ = std::fs::remove_file(path);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_home_without_home_variables_uses_secure_fallback() {
        const CHILD_ENV: &str = "ZO_TEST_CONFIG_TOOL_NO_HOME_CHILD";
        if std::env::var_os(CHILD_ENV).is_some() {
            let home = config_home_dir();
            assert!(home.is_absolute());
            assert_eq!(home.file_name(), Some(std::ffi::OsStr::new(".zo")));
            assert!(home.is_dir());
            return;
        }

        let status = std::process::Command::new(std::env::current_exe().expect("test binary"))
            .arg("config_home_without_home_variables_uses_secure_fallback")
            .arg("--nocapture")
            .env(CHILD_ENV, "1")
            .env_remove(core_types::paths::ZO_CONFIG_HOME_ENV)
            .env_remove(core_types::paths::ZO_HOME_ENV)
            .env_remove("HOME")
            .status()
            .expect("run isolated no-home config test");
        assert!(status.success(), "isolated no-home config test failed");
    }

    #[test]
    fn plan_artifact_id_is_stable_and_content_addressed() {
        let id = plan_artifact_id("step 1\nstep 2", Some("two steps"));
        assert_eq!(
            id,
            plan_artifact_id("step 1\nstep 2", Some("two steps")),
            "same content → same id (idempotent overwrite)"
        );
        assert_ne!(
            id,
            plan_artifact_id("step 1\nstep 3", Some("two steps")),
            "different plan → different id"
        );
        assert_ne!(
            id,
            plan_artifact_id("step 1\nstep 2", Some("other summary")),
            "different summary → different id"
        );
        assert!(id.starts_with("plan-"), "got {id}");
    }

    #[test]
    fn prune_plan_artifacts_bounds_dir_and_keeps_current_and_siblings() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "zo-plan-prune-{}-{nanos}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");

        // 23 plan artifacts (over the retention bound), one non-plan sibling.
        for i in 0..23u32 {
            std::fs::write(dir.join(format!("plan-{i:016x}.md")), b"# Plan\n\nx\n")
                .expect("write artifact");
        }
        let sibling = dir.join("notes.md");
        std::fs::write(&sibling, b"keep me").expect("write sibling");
        let current = dir.join("plan-000000000000ffff.md");
        std::fs::write(&current, b"# Current\n\nnow\n").expect("write current");

        prune_plan_artifacts(&dir, PLAN_ARTIFACT_RETENTION, Some(&current));

        let remaining: Vec<_> = std::fs::read_dir(&dir)
            .expect("read dir")
            .flatten()
            .map(|entry| entry.path())
            .collect();
        let plan_count = remaining.iter().filter(|path| is_plan_artifact(path)).count();
        let remaining: Vec<_> = remaining
            .iter()
            .filter_map(|path| path.file_name()?.to_str().map(str::to_owned))
            .collect();
        assert_eq!(
            plan_count, PLAN_ARTIFACT_RETENTION,
            "directory is bounded to the retention limit: {remaining:?}"
        );
        assert!(
            remaining.iter().any(|name| name == "plan-000000000000ffff.md"),
            "the just-written artifact is never pruned: {remaining:?}"
        );
        assert!(
            remaining.iter().any(|name| name == "notes.md"),
            "non-plan siblings are left untouched: {remaining:?}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn format_plan_markdown_titles_from_summary_else_default() {
        let titled = format_plan_markdown("do X", Some("My Plan"));
        assert!(titled.starts_with("# My Plan\n"), "got {titled}");
        assert!(titled.contains("do X"));
        assert!(format_plan_markdown("do X", None).starts_with("# Plan\n"));
        assert!(
            format_plan_markdown("do X", Some("   ")).starts_with("# Plan\n"),
            "blank summary falls back to the default title"
        );
    }

    #[test]
    fn global_config_deep_merges_inherited_roots_and_writes_only_primary() {
        let _guard = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system clock")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "zo-config-tools-global-roots-{}-{nanos}",
            std::process::id()
        ));
        let primary = root.join("primary");
        let secondary = root.join("secondary");
        let user_home = root.join("user-home");
        let fallback = user_home.join(".zo");
        for directory in [&primary, &secondary, &fallback] {
            std::fs::create_dir_all(directory).expect("create config root");
        }
        std::fs::write(
            fallback.join("settings.json"),
            r#"{"theme":"fallback","nested":{"fallback":true,"shared":"fallback"}}"#,
        )
        .expect("write fallback settings");
        std::fs::write(
            secondary.join("settings.json"),
            r#"{"nested":{"secondary":true,"shared":"secondary"}}"#,
        )
        .expect("write secondary settings");
        std::fs::write(
            primary.join("settings.json"),
            r#"{"nested":{"primary":true,"shared":"primary"}}"#,
        )
        .expect("write primary settings");

        let previous_config_home = std::env::var_os("ZO_CONFIG_HOME");
        let previous_zo_home = std::env::var_os("ZO_HOME");
        let previous_home = std::env::var_os("HOME");
        std::env::set_var("ZO_CONFIG_HOME", &primary);
        std::env::set_var("ZO_HOME", &secondary);
        std::env::set_var("HOME", &user_home);

        let merged = read_global_config_object().expect("read merged global settings");
        assert_eq!(
            get_nested_value(&merged, &["nested", "fallback"]),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            get_nested_value(&merged, &["nested", "secondary"]),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            get_nested_value(&merged, &["nested", "primary"]),
            Some(&Value::Bool(true))
        );
        assert_eq!(
            get_nested_value(&merged, &["nested", "shared"]),
            Some(&Value::String(String::from("primary")))
        );

        let get = execute_config(ConfigInput {
            setting: String::from("theme"),
            value: None,
        })
        .expect("get inherited global setting");
        assert_eq!(get.value, Some(Value::String(String::from("fallback"))));

        let set = execute_config(ConfigInput {
            setting: String::from("theme"),
            value: Some(ConfigValue::String(String::from("primary-theme"))),
        })
        .expect("set global setting");
        assert_eq!(
            set.previous_value,
            Some(Value::String(String::from("fallback")))
        );
        assert_eq!(
            set.new_value,
            Some(Value::String(String::from("primary-theme")))
        );

        let primary_after = read_json_object(&primary.join("settings.json"))
            .expect("read primary settings after write");
        assert_eq!(
            get_nested_value(&primary_after, &["theme"]),
            Some(&Value::String(String::from("primary-theme")))
        );
        let secondary_after = read_json_object(&secondary.join("settings.json")).expect("read secondary settings after write");
        let fallback_after = read_json_object(&fallback.join("settings.json"))
            .expect("read fallback settings after write");
        assert!(get_nested_value(&secondary_after, &["theme"]).is_none());
        assert_eq!(
            get_nested_value(&fallback_after, &["theme"]),
            Some(&Value::String(String::from("fallback")))
        );

        for (name, previous) in [
            ("ZO_CONFIG_HOME", previous_config_home),
            ("ZO_HOME", previous_zo_home),
            ("HOME", previous_home),
        ] {
            match previous {
                Some(value) => std::env::set_var(name, value),
                None => std::env::remove_var(name),
            }
        }
        std::fs::remove_dir_all(root).expect("remove temporary config roots");
    }

    #[test]
    fn reasoning_effort_config_setting_accepts_the_full_p9_ladder() {
        // Regression: the `/config` tool's `reasoningEffort` allowlist is a
        // separate static list from `Effort::from_token` — P9 renamed
        // `ultracode` to `smart` and gave `ultra` its own static level, and
        // this list silently still only offered the pre-rename vocabulary
        // (which meant the config tool could never *write* "ultra" or
        // "smart", only read them back if hand-edited into settings.json).
        let spec = supported_config_setting("reasoningEffort").expect("known setting");
        for accepted in [
            "off", "low", "medium", "high", "xhigh", "max", "ultra", "smart",
        ] {
            normalize_config_value(spec, ConfigValue::String(accepted.to_string()))
                .unwrap_or_else(|error| panic!("{accepted} should be accepted: {error}"));
        }
        // Legacy alias: pre-rename persisted/scripted value must keep working.
        normalize_config_value(spec, ConfigValue::String("ultracode".to_string()))
            .expect("legacy `ultracode` token stays accepted for backward compatibility");
        let error = normalize_config_value(spec, ConfigValue::String("bogus".to_string()))
            .expect_err("unknown effort token is rejected");
        assert!(error.contains("bogus"), "error: {error}");
    }
}
