//! Persistent model and reasoning-effort preferences for the plain `zo` CLI.
//!
//! The preference file deliberately lives beside the other user settings in
//! the canonical Zo home (`ZO_CONFIG_HOME`, then `ZO_HOME`, then `~/.zo`).
//! Keeping the two selected values in that document means the runtime's
//! existing config loader sees the same defaults as the plain front-end, while
//! unknown settings remain untouched when a preference is changed.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::effort::Effort;

/// The user settings document that carries the two persistent defaults.
pub const PREFERENCES_FILE_NAME: &str = "settings.json";
const MODEL_KEY: &str = "model";
const EFFORT_KEY: &str = "reasoningEffort";
const WORKSPACE_TRUST_KEY: &str = "workspaceTrust";
const TOOL_DIGEST_KEY: &str = "toolDigest";
/// Whether the interactive front shows thinking cells (t-5872). Absent means
/// the front's own default (`tui::thinking::SHOW_BY_DEFAULT`); `/thinking`
/// toggles the running session without writing here.
pub const SHOW_THINKING_KEY: &str = "showThinking";

/// The one recognized `workspaceTrust` value: the interactive trust gate is
/// retired and every folder opens in full access, no prompt, outranking even
/// a folder's previously recorded decision.
pub const ALWAYS_FULL_WORKSPACE_TRUST: &str = "always-full";

/// A best-effort snapshot of the persisted startup choices.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Preferences {
    pub model: Option<String>,
    pub effort: Option<Effort>,
    /// Raw `workspaceTrust` value; only [`ALWAYS_FULL_WORKSPACE_TRUST`] has
    /// meaning today. Kept as the raw string so an unrecognized value keeps
    /// the gate instead of failing the whole settings load.
    pub workspace_trust: Option<String>,
    /// Raw `toolDigest` value (`off` turns the over-cap fold off; anything
    /// else keeps the default on). Parsed by `tools::ToolDigestMode`.
    pub tool_digest: Option<String>,
    /// `showThinking`, when the document says one way or the other.
    pub show_thinking: Option<bool>,
}

impl Preferences {
    /// True when the user asked for every folder to open in full access with
    /// no trust prompt (`"workspaceTrust": "always-full"`).
    #[must_use]
    pub fn grants_full_trust_everywhere(&self) -> bool {
        self.workspace_trust.as_deref() == Some(ALWAYS_FULL_WORKSPACE_TRUST)
    }
}

/// Resolve the canonical preference path without creating anything.
#[must_use]
pub fn preferences_path() -> PathBuf {
    runtime::default_config_home().join(PREFERENCES_FILE_NAME)
}

/// Load the preferences from the canonical user settings file.
///
/// A missing, malformed, or non-object document is intentionally treated as
/// empty. Startup must retain its safe defaults when a settings file is
/// damaged, and a later explicit choice can repair just the known fields.
#[must_use]
pub fn load() -> Preferences {
    load_from_path(&preferences_path())
}

/// Load preferences from an explicit path. This is public so callers and
/// hermetic tests can use the same parser without changing process-global
/// environment variables.
#[must_use]
pub fn load_from_path(path: &Path) -> Preferences {
    let Ok(contents) = fs::read_to_string(path) else {
        return Preferences::default();
    };
    let Ok(value) = serde_json::from_str::<Value>(&contents) else {
        return Preferences::default();
    };
    let Some(object) = value.as_object() else {
        return Preferences::default();
    };

    Preferences {
        model: object
            .get(MODEL_KEY)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(crate::cli_args::resolve_model_alias),
        effort: object
            .get(EFFORT_KEY)
            .and_then(Value::as_str)
            .and_then(Effort::from_token),
        workspace_trust: object
            .get(WORKSPACE_TRUST_KEY)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        tool_digest: object
            .get(TOOL_DIGEST_KEY)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string),
        show_thinking: object.get(SHOW_THINKING_KEY).and_then(Value::as_bool),
    }
}

/// Persist both startup choices in the canonical user settings document.
///
/// The existing JSON object is merged rather than replaced, and publication
/// goes through Zo's atomic writer so a process interruption cannot leave a
/// half-written settings file. If the old document is corrupt, a fresh object
/// containing the explicit choices is written.
pub fn save(model: &str, effort: Effort) -> io::Result<()> {
    save_to_path(&preferences_path(), model, effort)
}

/// Persist choices to a specific settings path. The path's parent is created
/// on demand; this is the only write side effect of this module.
///
/// A family head is stored by its alias (`opus`, not `claude-opus-5`): the
/// alias follows the family when the provider ships the next release, while
/// a release id would freeze the choice on the day it was made.
pub fn save_to_path(path: &Path, model: &str, effort: Effort) -> io::Result<()> {
    let _guard = runtime::file_ops::SettingsFileLock::acquire(path)?;
    let mut object = read_object(path);
    let model = pinned_model_id(&persistable_model_id(model));
    if object.get(MODEL_KEY).and_then(Value::as_str) != Some(&model) {
        if let Some(smart) = object.get_mut("smart").and_then(Value::as_object_mut) {
            smart.remove(judgment_provider::connections::CODING_CONNECTION_KEY);
            smart.remove(judgment_provider::connections::PREVIOUS_CODING_KEY);
        }
    }
    object.insert(MODEL_KEY.to_string(), Value::String(model));
    object.insert(
        EFFORT_KEY.to_string(),
        Value::String(effort.canonical().to_string()),
    );
    let payload = serde_json::to_vec_pretty(&Value::Object(object)).map_err(io::Error::other)?;
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    crate::write_atomic(path, &payload)
}

fn read_object(path: &Path) -> Map<String, Value> {
    fs::read_to_string(path)
        .ok()
        .and_then(|contents| serde_json::from_str::<Value>(&contents).ok())
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default()
}

/// `/fast` is a session-only serving tier. Store the bare family so a future
/// launch does not silently inherit a transient speed choice.
#[must_use]
pub fn persistable_model_id(model: &str) -> String {
    api::openai_fast_variant_pair(model)
        .map_or_else(|| model.to_string(), |(bare, _fast)| bare)
}

/// The value settings should carry for `model`: its family alias when it is
/// a family head, otherwise the id itself. A `provider/model` reference and a
/// model no alias names are stored as written.
#[must_use]
pub fn pinned_model_id(model: &str) -> String {
    let trimmed = model.trim();
    if trimmed.contains('/') {
        return trimmed.to_string();
    }
    api::family_alias_for(trimmed).map_or_else(|| trimmed.to_string(), str::to_string)
}

/// Settings key recording every automatic rewrite of `model`, newest last.
pub const MODEL_MIGRATIONS_KEY: &str = "modelMigrations";

/// One automatic rewrite of the persisted model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelMigration {
    pub from: String,
    pub to: String,
}

/// Pin the persisted model to its family alias, once.
///
/// A settings file written before aliases were pinned carries a release id
/// (`claude-opus-5`). Left alone it would keep that release forever while the
/// family moved on. Rewritten to `opus` it follows — and the rewrite is
/// recorded under `modelMigrations` so the person can see what happened and
/// when. `None` when nothing needed rewriting.
pub fn pin_persisted_model_alias() -> io::Result<Option<ModelMigration>> {
    pin_persisted_model_alias_at(&preferences_path())
}

pub fn pin_persisted_model_alias_at(path: &Path) -> io::Result<Option<ModelMigration>> {
    let _guard = runtime::file_ops::SettingsFileLock::acquire(path)?;
    let mut object = read_object(path);
    let Some(current) = object
        .get(MODEL_KEY)
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|model| !model.is_empty())
        .map(str::to_string)
    else {
        return Ok(None);
    };
    let pinned = pinned_model_id(&current);
    if pinned.eq_ignore_ascii_case(&current) {
        return Ok(None);
    }
    object.insert(MODEL_KEY.to_string(), Value::String(pinned.clone()));
    let at = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs());
    let record = serde_json::json!({
        "from": current,
        "to": pinned,
        "at": at,
        "reason": "family-alias-pin",
    });
    match object.get_mut(MODEL_MIGRATIONS_KEY) {
        Some(Value::Array(records)) => records.push(record),
        _ => {
            object.insert(MODEL_MIGRATIONS_KEY.to_string(), Value::Array(vec![record]));
        }
    }
    let payload = serde_json::to_vec_pretty(&Value::Object(object)).map_err(io::Error::other)?;
    if let Some(parent) = path.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    crate::write_atomic(path, &payload)?;
    Ok(Some(ModelMigration {
        from: current,
        to: pinned,
    }))
}

#[cfg(test)]
mod tests {
    use super::{
        load_from_path, persistable_model_id, save_to_path, Preferences, EFFORT_KEY,
    };
    use crate::effort::Effort;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn path(label: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "zo-preferences-{label}-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("create test directory");
        path.join("settings.json")
    }

    #[test]
    fn missing_and_corrupt_documents_use_safe_empty_preferences() {
        let missing = path("missing");
        assert_eq!(load_from_path(&missing), Preferences::default());
        fs::write(&missing, b"not json").expect("write corrupt settings");
        assert_eq!(load_from_path(&missing), Preferences::default());
        let _ = fs::remove_dir_all(missing.parent().expect("test parent"));
    }

    #[test]
    fn preferences_writers_preserve_a_document_locked_by_another_writer() {
        let settings = path("shared-lock");
        let original = br#"{"model":"fixture/model","smart":{"retained":true}}"#;
        fs::write(&settings, original).expect("seed");
        let lock = settings.with_extension("json.lock");
        let owner = format!("{}\n", std::process::id());
        fs::write(&lock, &owner).expect("lock");
        let save = save_to_path(&settings, "replacement/model", Effort::Smart);
        assert_eq!(save.expect_err("must respect the other writer").kind(), std::io::ErrorKind::WouldBlock);
        let pin = super::pin_persisted_model_alias_at(&settings);
        assert_eq!(pin.expect_err("migration must also lock before reading").kind(), std::io::ErrorKind::WouldBlock);
        assert_eq!(fs::read(&settings).expect("read"), original);
        assert_eq!(fs::read_to_string(&lock).expect("owner"), owner);
        fs::remove_dir_all(settings.parent().expect("parent")).expect("cleanup");
    }

    #[test]
    fn preferences_round_trip_and_preserve_unrelated_settings() {
        let _lock = crate::test_env_lock();
        let settings = path("round-trip");
        fs::write(&settings, r#"{"mcpServers":{},"custom":true}"#)
            .expect("seed settings");
        save_to_path(&settings, api::ANTHROPIC_OPUS_MODEL_ALIAS, Effort::Smart)
            .expect("save preferences");
        let loaded = load_from_path(&settings);
        let expected = api::resolve_catalog_alias(api::ANTHROPIC_OPUS_MODEL_ALIAS);
        assert_eq!(loaded.model.as_deref(), Some(expected.as_str()));
        assert_eq!(loaded.effort, Some(Effort::Smart));
        let document: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings).expect("read settings"))
                .expect("valid settings");
        assert_eq!(document["custom"], true);
        assert_eq!(document[EFFORT_KEY], "smart");
        let _ = fs::remove_dir_all(settings.parent().expect("test parent"));
    }

    /// A family head is persisted by its alias, and a release id already on
    /// disk is pinned to the alias once, with a migration record.
    #[test]
    fn family_heads_are_pinned_to_their_alias() {
        let _lock = crate::test_env_lock();
        let opus = api::resolve_catalog_alias(api::ANTHROPIC_OPUS_MODEL_ALIAS);
        assert_eq!(super::pinned_model_id(&opus), "opus");
        assert_eq!(super::pinned_model_id("claude-opus-4-8"), "claude-opus-4-8", "not a family head");
        assert_eq!(super::pinned_model_id("openai/gpt-5.6-terra"), "openai/gpt-5.6-terra");

        let settings = path("pin");
        save_to_path(&settings, &opus, Effort::Smart).expect("save");
        let document: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(document["model"], "opus");
        assert_eq!(load_from_path(&settings).model.as_deref(), Some(opus.as_str()));

        fs::write(&settings, format!(r#"{{"model":"{opus}","custom":true}}"#)).unwrap();
        let migration = super::pin_persisted_model_alias_at(&settings).expect("pin").expect("rewritten");
        assert_eq!(migration.from, opus);
        assert_eq!(migration.to, "opus");
        let document: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&settings).unwrap()).unwrap();
        assert_eq!(document["model"], "opus");
        assert_eq!(document["custom"], true);
        assert_eq!(document[super::MODEL_MIGRATIONS_KEY][0]["to"], "opus");
        assert_eq!(
            super::pin_persisted_model_alias_at(&settings).expect("pin again"),
            None,
            "the second pass has nothing to do"
        );
        let _ = fs::remove_dir_all(settings.parent().expect("test parent"));
    }

    #[test]
    fn persisted_fast_models_are_normalized_to_the_bare_family() {
        let _lock = crate::test_env_lock();
        let _catalog =
            crate::support::EnvVarGuard::set(api::MODEL_CONTEXT_WINDOWS_ENV, None);
        let current = api::builtin_provider_catalog()
            .iter()
            .find(|entry| entry.alias == api::OPENAI_LATEST_MODEL_ALIAS)
            .expect("shipped current OpenAI model")
            .canonical_model_id;
        let (bare, fast) =
            api::openai_fast_variant_pair(current).expect("current fast service-tier pair");
        assert_eq!(
            persistable_model_id(&fast),
            bare
        );
        assert_eq!(persistable_model_id(current), current);
        let anthropic = api::resolve_catalog_alias(api::ANTHROPIC_OPUS_MODEL_ALIAS);
        assert_eq!(persistable_model_id(&anthropic), anthropic);
    }

    /// Marks a run of this test binary as a child of
    /// [`one_catalog_view_from_process_start_in_either_order`], naming the
    /// order it asks in.
    const CATALOG_VIEW_ORDER_ENV: &str = "ZO_TEST_CATALOG_VIEW_ORDER";
    /// The child asks the catalog first, then reads the catalog file.
    const START_FIRST: &str = "start-first";
    /// The child reads the catalog file first, then asks.
    const CATALOG_FIRST: &str = "catalog-first";
    /// The marker before a child's answer on its stdout.
    const CATALOG_VIEW_LINE: &str = "catalog-view ";

    /// What the catalog answers: where the opus alias resolves, the alias that
    /// id is pinned to, every family alias with its head, and each provider's
    /// latest model.
    fn catalog_view() -> serde_json::Value {
        let opus = api::resolve_catalog_alias(api::ANTHROPIC_OPUS_MODEL_ALIAS);
        let heads: std::collections::BTreeMap<&str, &str> = api::provider_catalog()
            .iter()
            .filter(|entry| api::family_alias_for(entry.canonical_model_id) == Some(entry.alias))
            .map(|entry| (entry.alias, entry.canonical_model_id))
            .collect();
        let latest: Vec<Option<&str>> = [
            api::ProviderKind::Anthropic,
            api::ProviderKind::OpenAi,
            api::ProviderKind::Google,
            api::ProviderKind::Xai,
        ]
        .into_iter()
        .map(api::latest_model_for_provider)
        .collect();
        serde_json::json!({
            "opus": opus,
            "pinned": super::pinned_model_id(&opus),
            "heads": heads,
            "latest": latest,
        })
    }

    /// Write `home`'s catalog file the way an overlay moves a family: the opus
    /// alias on another shipped release whose id carries the family's name.
    /// Read from the shipped catalog's own bytes, so nothing here asks the live
    /// catalog before the file exists. Returns that release.
    fn declare_another_opus_release(home: &Path) -> String {
        let shipped: serde_json::Value =
            serde_json::from_str(api::builtin_model_catalog_json()).expect("shipped catalog");
        let row = shipped["aliases"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["alias"] == api::ANTHROPIC_OPUS_MODEL_ALIAS))
            .expect("the shipped opus row");
        let head = row["canonical"].as_str().expect("the shipped opus head");
        let release = shipped["models"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|model| model["provider"] == row["provider"])
            .flat_map(|model| model["ids"].as_array().into_iter().flatten())
            .filter_map(serde_json::Value::as_str)
            .find(|id| *id != head && id.contains(api::ANTHROPIC_OPUS_MODEL_ALIAS))
            .expect("another shipped opus release")
            .to_string();
        let overlay = serde_json::json!({
            "aliases": [{
                "alias": api::ANTHROPIC_OPUS_MODEL_ALIAS,
                "canonical": release,
                "provider": row["provider"],
            }],
        });
        fs::create_dir_all(home).expect("config home");
        fs::write(home.join(runtime::model_catalog::CATALOG_FILE), overlay.to_string())
            .expect("write the catalog file");
        release
    }

    /// A child's side: in its own test home, declare the moved release, then
    /// print what the catalog answers — at process start when the order asks
    /// first, and after the catalog file is read.
    fn answer_as_a_catalog_view_child(order: &str) {
        let _lock = crate::test_env_lock();
        let home = runtime::default_config_home();
        let release = declare_another_opus_release(&home);
        let mut views = Vec::new();
        if order == START_FIRST {
            views.push(("start", catalog_view()));
        }
        crate::runtime_support::publish_model_catalog();
        views.push(("catalog", catalog_view()));
        for (phase, view) in views {
            println!(
                "{CATALOG_VIEW_LINE}{}",
                serde_json::json!({ "phase": phase, "declared": release, "view": view })
            );
        }
        let _ = fs::remove_file(home.join(runtime::model_catalog::CATALOG_FILE));
    }

    /// The views a fresh run of this test binary, on this test alone, reports
    /// when it asks in `order`. Its home is temporary and it inherits no
    /// catalog from this process's environment.
    fn catalog_views_of_a_child(order: &str) -> Vec<serde_json::Value> {
        let home = crate::support::temp_dir("catalog-view-child");
        let module = module_path!()
            .split_once("::")
            .map_or(module_path!(), |(_crate, path)| path);
        let filter = format!("{module}::one_catalog_view_from_process_start_in_either_order");
        let output = std::process::Command::new(std::env::current_exe().expect("this test binary"))
            .arg(&filter)
            .args(["--exact", "--nocapture", "--test-threads=1"])
            .env(CATALOG_VIEW_ORDER_ENV, order)
            .env("HOME", &home)
            .env("ZO_CONFIG_HOME", &home)
            .env("ZO_HOME", &home)
            .env("ZO_DISABLE_KEYCHAIN", "1")
            .env("ZO_DISABLE_MODEL_DISCOVERY", "1")
            .env_remove(api::MODEL_CONTEXT_WINDOWS_ENV)
            .env_remove(api::MODEL_EFFORT_CEILINGS_ENV)
            .env_remove(api::CUSTOM_PROVIDERS_ENV)
            .output()
            .expect("run a child");
        let _ = fs::remove_dir_all(&home);
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            output.status.success(),
            "the {order} child failed:\n{stdout}\n{}",
            String::from_utf8_lossy(&output.stderr)
        );
        // libtest names the test (`test … ... `) on the line the child's
        // first answer lands on, so the marker is looked for, not anchored.
        stdout
            .lines()
            .filter_map(|line| line.split_once(CATALOG_VIEW_LINE).map(|(_, view)| view))
            .map(|line| serde_json::from_str(line).expect("a view line"))
            .collect()
    }

    /// One catalog per process (t-44087): what the catalog answers at process
    /// start — where the opus alias resolves, the alias it is pinned to, the
    /// family heads — is what it answers once the catalog file is read, and
    /// reading the file first changes nothing. Each order runs in a fresh run
    /// of this binary in a temporary home: the catalog is process-global, so
    /// only a new process can be asked at its start.
    #[test]
    fn one_catalog_view_from_process_start_in_either_order() {
        if let Ok(order) = std::env::var(CATALOG_VIEW_ORDER_ENV) {
            answer_as_a_catalog_view_child(&order);
            return;
        }
        let start_first = catalog_views_of_a_child(START_FIRST);
        let catalog_first = catalog_views_of_a_child(CATALOG_FIRST);
        let [at_start, after_reading] = start_first.as_slice() else {
            panic!("two views from the {START_FIRST} child: {start_first:?}");
        };
        let [read_first] = catalog_first.as_slice() else {
            panic!("one view from the {CATALOG_FIRST} child: {catalog_first:?}");
        };
        assert_eq!(
            after_reading["view"]["opus"], after_reading["declared"],
            "the catalog file was read: {after_reading}"
        );
        assert_eq!(
            at_start["view"], after_reading["view"],
            "process start answered from another catalog than the one the file declares"
        );
        assert_eq!(
            read_first["view"], after_reading["view"],
            "reading the catalog file first changes nothing"
        );
    }
}
