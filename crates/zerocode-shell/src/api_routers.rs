//! API router presets, Keychain secret management, and ~/.zo/settings.json providers[] upsert.
//!
//! §1.0 contract:
//! - ONE providers table: ~/.zo/settings.json providers[]; the window writes, zo reads.
//! - Presets are DATA rows loaded from `api-routers.json`.
//! - NO code branch on router names anywhere.
//! - Key goes to keychain under `dev.zerocode.router.<id>` via `accounts.rs`.
//! - Settings carries only auth_env name.
//! - Read-modify-write on ~/.zo/settings.json preserves all other keys and order.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub static API_ROUTERS_PRESETS_JSON: &str = include_str!("api-routers.json");

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouterAuth {
    pub header: String,
    pub scheme: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouterModelsConfig {
    pub path: String,
    pub id_field: String,
    #[serde(default)]
    pub context_field: Option<String>,
    #[serde(default)]
    pub pricing_field: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouterUsageConfig {
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub limit_field: Option<String>,
    #[serde(default)]
    pub used_field: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RouterPreset {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub auth: RouterAuth,
    pub models: RouterModelsConfig,
    pub usage: RouterUsageConfig,
    /// Headers sent verbatim, for a gateway that gates on more than the key —
    /// including a `User-Agent` of its own, which overrides the identity the
    /// connection test would otherwise present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
    /// The client a gateway insists on seeing, by the name zo's
    /// `client_fingerprint` reads (`"claude"`). Only a gateway that refuses an
    /// unknown client before it weighs the key carries one — AgentRouter's
    /// `unauthorized_client_error` — so no other router is told zo is some
    /// other client. Written into that row's `providers[]` entry verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_fingerprint: Option<String>,
}

/// One model of a `providers[]` row: a plain id, or an object naming the id
/// with that model's own limits — the two shapes zo's `CustomModel` parses as
/// one list. The window writes the object only when the connection test read a
/// window for that model; a model the gateway said nothing about is a plain id
/// and reads the provider's.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum ProviderModel {
    Id(String),
    Stated {
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        context_window: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_output_tokens: Option<u64>,
    },
}

impl ProviderModel {
    /// The same model with only the limits it states (a zero states nothing),
    /// and a plain id when that leaves none.
    fn plain_when_silent(self) -> Self {
        let Self::Stated {
            id,
            context_window,
            max_output_tokens,
        } = self
        else {
            return self;
        };
        let stated = |value: Option<u64>| value.filter(|&value| value > 0);
        match (stated(context_window), stated(max_output_tokens)) {
            (None, None) => Self::Id(id),
            (context_window, max_output_tokens) => Self::Stated {
                id,
                context_window,
                max_output_tokens,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProviderEntry {
    pub name: String,
    pub base_url: String,
    #[serde(default)]
    pub models: Vec<ProviderModel>,
    pub requires_auth: bool,
    pub auth_env: String,
    /// A window for every model of the row that states none of its own. The
    /// window never writes one — it states each model's own — but a row may
    /// carry one from a hand or an older window.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_fingerprint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub headers: Option<std::collections::BTreeMap<String, String>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DiscoveredModel {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_length: Option<u64>,
}

/// The prefix every router key variable this window hands a zo pane carries —
/// the one mark that tells the window's own `providers[]` entries from the ones
/// zo's `/connect` wrote, whose keys are zo's business and are never touched.
pub const ROUTER_ENV_PREFIX: &str = zerocode_harness::ROUTER_KEY_ENV_PREFIX;

/// Keychain service for a router key, keyed by the `auth_env` name the shared
/// `providers[]` entry carries — the one identifier the save, the removal and a
/// zo launch all read off the row itself, without a second table.
pub fn router_keychain_service_for_env(auth_env: &str) -> String {
    format!(
        "{}{auth_env}",
        zerocode_harness::ROUTER_KEYCHAIN_SERVICE_PREFIX
    )
}

/// Whether `auth_env` is one of the window's own router variables — the mark
/// that separates a row this pane wrote from one zo's `/connect` wrote.
pub fn is_router_env(auth_env: &str) -> bool {
    auth_env.starts_with(ROUTER_ENV_PREFIX)
}

/// The router keys a zo launch carries: for each `providers[]` entry whose
/// `auth_env` is the window's own (the prefix), the key the keychain holds under
/// that name. An entry without a stored key is skipped — an empty variable would
/// read as "configured, empty". Pure over the reader, so it is testable without
/// a keychain.
pub fn router_launch_env(
    providers: &[ProviderEntry],
    read_key: impl Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    let mut env: Vec<(String, String)> = Vec::new();
    for entry in providers {
        if !is_router_env(&entry.auth_env) || env.iter().any(|(name, _)| name == &entry.auth_env) {
            continue;
        }
        let key = read_key(&router_keychain_service_for_env(&entry.auth_env))
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty());
        if let Some(key) = key {
            env.push((entry.auth_env.clone(), key));
        }
    }
    env
}

/// [`router_launch_env`] over zo's real settings file and this user's keychain.
pub fn router_launch_env_from_disk() -> Vec<(String, String)> {
    let Some(path) = zo_settings_path() else {
        return Vec::new();
    };
    let Ok(providers) = read_providers_file(&path) else {
        return Vec::new();
    };
    router_launch_env(&providers, |service| {
        crate::accounts::read_keychain_service(service).ok()
    })
}

/// The word a router variable is spelled from: ASCII letters and digits,
/// upper-cased, every other run of characters one `_`, none at either end. A
/// name with no ASCII letter in it (`사내 게이트웨이`) has no word at all.
fn router_env_word(source: &str) -> String {
    let mut word = String::new();
    for ch in source.chars() {
        if ch.is_ascii_alphanumeric() {
            word.push(ch.to_ascii_uppercase());
        } else if !word.is_empty() && !word.ends_with('_') {
            word.push('_');
        }
    }
    word.trim_end_matches('_').to_string()
}

/// The word a row whose name and preset give none is spelled from.
const UNNAMED_ROUTER_WORD: &str = "CUSTOM";

/// Environment variable name for a router key spelled from `source`.
pub fn router_auth_env_name(source: &str) -> String {
    let word = router_env_word(source);
    let word = if word.is_empty() {
        UNNAMED_ROUTER_WORD
    } else {
        word.as_str()
    };
    format!("{ROUTER_ENV_PREFIX}{word}_KEY")
}

/// One `providers[]` row as the identity rules see it: its name and the
/// variable it reads its key from. Read off the raw JSON, so a row zo's
/// `/connect` wrote in a shape this window does not parse still holds its name.
struct RowIdentity {
    name: String,
    auth_env: Option<String>,
}

fn row_identities(root: &serde_json::Map<String, serde_json::Value>) -> Vec<RowIdentity> {
    root.get("providers")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| {
            let name = row.get("name")?.as_str()?.to_string();
            let auth_env = row
                .get("auth_env")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string);
            Some(RowIdentity { name, auth_env })
        })
        .collect()
}

/// The variable a row saved under `name` reads its key from — decided once, at
/// its first save, and kept.
///
/// A row that already stands under this name keeps the router variable it was
/// given. A new row is spelled from its preset id when it was saved from one,
/// else from its name; when that spelling is taken by another row (a second
/// OpenRouter account, two Hangul names that spell nothing) the first free
/// `_2`, `_3`, … is used. No two rows ever name one variable, so no row can be
/// handed another row's key.
///
/// Rows an older window wrote can already share one variable; its key cannot
/// be told apart. A save that brings a key (`key_given`) moves its row to a
/// variable of its own; a keyless save keeps the shared one rather than lose
/// the only key the row can reach.
fn assign_auth_env(
    rows: &[RowIdentity],
    name: &str,
    preset_id: Option<&str>,
    key_given: bool,
) -> String {
    let taken = |auth_env: &str| {
        rows.iter()
            .any(|row| row.name != name && row.auth_env.as_deref() == Some(auth_env))
    };
    if let Some(own) = rows
        .iter()
        .find(|row| row.name == name)
        .and_then(|row| row.auth_env.as_deref())
        .filter(|auth_env| is_router_env(auth_env) && !(key_given && taken(auth_env)))
    {
        return own.to_string();
    }
    let source = preset_id
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .unwrap_or(name);
    let base = router_auth_env_name(source);
    let stem = base.trim_end_matches("_KEY");
    std::iter::once(base.clone())
        .chain((2..).map(|n| format!("{stem}_{n}_KEY")))
        .find(|candidate| !taken(candidate))
        .unwrap_or(base)
}

/// Why a router save was refused, in a word the pane translates.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum RouterRefusalKind {
    /// This machine has no keychain to keep a router key in.
    KeychainUnavailable,
    /// Anything else; the message says what.
    Failed,
}

/// A refused router save: `{ kind, message }` over the wire, the shape the
/// pane's `failureOf` reads. The kind decides the words the pane shows; the
/// message is the backend's own sentence for every other failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RouterRefusal {
    pub kind: RouterRefusalKind,
    pub message: String,
}

impl RouterRefusal {
    fn keychain_unavailable() -> Self {
        Self {
            kind: RouterRefusalKind::KeychainUnavailable,
            message: "이 컴퓨터에는 라우터 키를 보관할 키체인이 없어 저장하지 않았습니다"
                .to_string(),
        }
    }
}

impl From<String> for RouterRefusal {
    fn from(message: String) -> Self {
        Self {
            kind: RouterRefusalKind::Failed,
            message,
        }
    }
}

/// Where router keys are kept. The keychain on a Mac; a map under test, so the
/// identity rules are proven without touching a real keychain.
pub trait RouterKeys {
    fn read(&self, service: &str) -> Result<Option<String>, RouterRefusal>;
    fn write(&self, service: &str, secret: &str) -> Result<(), RouterRefusal>;
    fn delete(&self, service: &str) -> Result<(), RouterRefusal>;
}

/// This user's keychain, where there is one.
pub struct Keychain {
    /// Whether this machine has a keychain for router keys. Only macOS does:
    /// everywhere else the accounts code's keychain writes are no-ops by
    /// design — its files are the whole story — but a router row's file holds
    /// only the variable's name, so a key "kept" there would be kept nowhere.
    available: bool,
}

impl Keychain {
    pub fn of_this_machine() -> Self {
        Self {
            available: crate::accounts::KEYCHAIN_AVAILABLE,
        }
    }

    /// Whether a key given to this store is kept anywhere — what the pane's
    /// hint and key field say before a person types one.
    pub fn keeps_keys(&self) -> bool {
        self.available
    }

    /// A machine with no keychain — every build but macOS.
    #[cfg(test)]
    pub(crate) fn without_a_store() -> Self {
        Self { available: false }
    }
}

impl RouterKeys for Keychain {
    fn read(&self, service: &str) -> Result<Option<String>, RouterRefusal> {
        if !self.available {
            return Ok(None);
        }
        crate::accounts::read_keychain_service_if_present(service).map_err(RouterRefusal::from)
    }

    fn write(&self, service: &str, secret: &str) -> Result<(), RouterRefusal> {
        if !self.available {
            return Err(RouterRefusal::keychain_unavailable());
        }
        crate::accounts::write_keychain_service(service, secret).map_err(RouterRefusal::from)
    }

    fn delete(&self, service: &str) -> Result<(), RouterRefusal> {
        if self.available {
            crate::accounts::delete_keychain_service(service).map_err(RouterRefusal::from)?;
        }
        Ok(())
    }
}

/// One save from the router pane, as the pane holds it.
pub struct RouterSave {
    /// The preset the row was saved from, when it was — only the first save of
    /// a row reads it, to spell the row's variable.
    pub preset_id: Option<String>,
    pub name: String,
    pub base_url: String,
    pub key: Option<String>,
    /// Each checked model with the window the connection test read for it.
    pub models: Vec<ProviderModel>,
    pub client_fingerprint: Option<String>,
    pub headers: Option<std::collections::BTreeMap<String, String>>,
}

fn read_settings_text(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(format!("파일 읽기 실패: {err}")),
    }
}

fn settings_root(text: &str) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    if text.trim().is_empty() {
        return Ok(serde_json::Map::new());
    }
    let root: serde_json::Value =
        serde_json::from_str(text).map_err(|err| format!("설정 JSON 파싱 실패: {err}"))?;
    let serde_json::Value::Object(root) = root else {
        return Err("settings.json 루트가 JSON 객체가 아닙니다".to_string());
    };
    if root.get("providers").is_some_and(|rows| !rows.is_array()) {
        return Err("settings.json providers가 JSON 배열이 아닙니다".to_string());
    }
    Ok(root)
}

/// One window writer of zo's settings file at a time — router rows and the
/// TypeSafe switch alike. Each reads the whole file, changes its part and
/// replaces the file, so two at once would lose one change.
static ZO_SETTINGS_WRITES: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// zo's settings file as the object every window writer edits; empty when the
/// file is absent.
pub(crate) fn read_zo_settings_root(
    path: &Path,
) -> Result<serde_json::Map<String, serde_json::Value>, String> {
    settings_root(&read_settings_text(path)?)
}

/// The file's bytes for `root`: pretty, with the trailing newline every writer
/// of it keeps.
fn render_settings_root(
    root: &serde_json::Map<String, serde_json::Value>,
) -> Result<String, String> {
    let mut rendered = serde_json::to_string_pretty(root).map_err(|err| err.to_string())?;
    rendered.push('\n');
    Ok(rendered)
}

/// Change zo's settings file under [`ZO_SETTINGS_WRITES`], keeping every key the
/// change does not touch as it stood. A change that refuses writes nothing.
pub(crate) fn update_zo_settings_root(
    path: &Path,
    change: impl FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> Result<(), String>,
) -> Result<(), String> {
    let _guard = ZO_SETTINGS_WRITES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut root = read_zo_settings_root(path)?;
    change(&mut root)?;
    crate::durable_file::replace_bytes(path, render_settings_root(&root)?.as_bytes())
        .map(|_| ())
        .map_err(|err| format!("파일 쓰기 실패: {err}"))
}

/// Save one router row: the key goes under the row's own variable, the row
/// into `providers[]`. A key the machine cannot keep refuses the whole save,
/// before anything is written.
pub fn save_router(
    path: &Path,
    save: RouterSave,
    keys: &dyn RouterKeys,
) -> Result<Vec<ProviderEntry>, RouterRefusal> {
    let _guard = ZO_SETTINGS_WRITES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    save_router_with_writer(path, save, keys, |path, updated| {
        crate::durable_file::replace_bytes(path, updated)
            .map(|_| ())
            .map_err(|error| error.to_string())
    })
}

fn save_router_with_writer(
    path: &Path,
    save: RouterSave,
    keys: &dyn RouterKeys,
    write: impl FnOnce(&Path, &[u8]) -> Result<(), String>,
) -> Result<Vec<ProviderEntry>, RouterRefusal> {
    let name = save.name.trim().to_string();
    if name.is_empty() {
        return Err("라우터 이름이 비어 있습니다".to_string().into());
    }
    let existing = read_settings_text(path)?;
    let rows = row_identities(&settings_root(&existing)?);
    let secret = save
        .key
        .as_deref()
        .map(str::trim)
        .filter(|key| !key.is_empty());
    let auth_env = assign_auth_env(&rows, &name, save.preset_id.as_deref(), secret.is_some());
    let service = router_keychain_service_for_env(&auth_env);
    let first_save = !rows
        .iter()
        .any(|row| row.name == name && row.auth_env.as_deref() == Some(auth_env.as_str()));
    // Whether the row has a key — given now, or stored for its variable — is
    // what `requires_auth` says: a keyless row (Ollama, OpenRouter saved
    // without one) written as needing a key is a row zo refuses to build.
    let previous_key = keys.read(&service)?;
    let requires_auth = secret.is_some()
        || (!first_save
            && previous_key
                .as_ref()
                .is_some_and(|key| !key.trim().is_empty()));
    let entry = ProviderEntry {
        name,
        base_url: save.base_url,
        // Each model states its own window; none speaks for all of them.
        models: save
            .models
            .into_iter()
            .map(ProviderModel::plain_when_silent)
            .collect(),
        requires_auth,
        auth_env,
        context_window: None,
        // The client the row names (its preset's data), in the word zo reads,
        // so the pane presents the identity the connection test passed under.
        // A row that names none is written as the generic client it is.
        client_fingerprint: save
            .client_fingerprint
            .map(|name| name.trim().to_string())
            .filter(|name| !name.is_empty()),
        headers: save.headers,
    };
    let updated = upsert_provider_json_str(&existing, &entry)?;
    let changing_key = secret.is_some() || (first_save && previous_key.is_some());
    let changed = match secret {
        Some(secret) => keys.write(&service, secret),
        None if changing_key => keys.delete(&service),
        None => Ok(()),
    };
    if let Err(mut error) =
        changed.and_then(|()| write(path, updated.as_bytes()).map_err(RouterRefusal::from))
    {
        if changing_key {
            let restored = match previous_key.as_deref() {
                Some(previous) => keys.write(&service, previous),
                None => keys.delete(&service),
            };
            if let Err(rollback) = restored {
                error
                    .message
                    .push_str(&format!("; 키 복구도 실패했습니다: {}", rollback.message));
            }
        }
        return Err(error);
    }
    Ok(read_providers_from_json_str(&updated)?)
}

/// Remove one router row, and the key it read — named by the row's own stored
/// variable, and only when no other row still reads that variable.
pub fn remove_router(
    path: &Path,
    name: &str,
    keys: &dyn RouterKeys,
) -> Result<Vec<ProviderEntry>, String> {
    let _guard = ZO_SETTINGS_WRITES
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let existing = read_settings_text(path)?;
    let rows = row_identities(&settings_root(&existing)?);
    let own_env = rows
        .iter()
        .find(|row| row.name == name)
        .and_then(|row| row.auth_env.clone())
        .filter(|auth_env| is_router_env(auth_env));
    remove_provider_file(path, name)?;
    if let Some(auth_env) = own_env
        && !rows
            .iter()
            .any(|row| row.name != name && row.auth_env.as_deref() == Some(auth_env.as_str()))
        && let Err(error) = keys.delete(&router_keychain_service_for_env(&auth_env))
    {
        crate::durable_file::replace_bytes(path, existing.as_bytes()).map_err(|rollback| {
            format!("{}; 설정 복구도 실패했습니다: {rollback}", error.message)
        })?;
        return Err(error.message);
    }
    read_providers_file(path)
}

/// Path to ~/.zo/settings.json.
pub fn zo_settings_path() -> Option<PathBuf> {
    zo_settings_path_from(
        std::env::var_os("ZO_CONFIG_HOME").as_deref().map(Path::new),
        std::env::var_os("ZO_HOME").as_deref().map(Path::new),
        dirs::home_dir().as_deref(),
    )
}

/// zo's own order for its config home: `ZO_CONFIG_HOME`, then `ZO_HOME`, then
/// `~/.zo` — the window must write where zo will read.
pub fn zo_settings_path_from(
    config_home: Option<&Path>,
    zo_home: Option<&Path>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    config_home
        .map(Path::to_path_buf)
        .or_else(|| zo_home.map(Path::to_path_buf))
        .or_else(|| home.map(|home| home.join(".zo")))
        .map(|root| root.join("settings.json"))
}

/// Parse default preset list.
pub fn load_presets() -> Result<Vec<RouterPreset>, String> {
    serde_json::from_str(API_ROUTERS_PRESETS_JSON).map_err(|err| err.to_string())
}

/// Read all provider entries from a settings JSON string.
pub fn read_providers_from_json_str(json_str: &str) -> Result<Vec<ProviderEntry>, String> {
    if json_str.trim().is_empty() {
        return Ok(Vec::new());
    }
    let value: serde_json::Value =
        serde_json::from_str(json_str).map_err(|err| format!("설정 JSON 파싱 실패: {err}"))?;
    let Some(providers_value) = value.get("providers") else {
        return Ok(Vec::new());
    };
    if let Some(list) = providers_value.as_array() {
        let mut entries = Vec::new();
        for item in list {
            if let Ok(entry) = serde_json::from_value::<ProviderEntry>(item.clone()) {
                entries.push(entry);
            }
        }
        Ok(entries)
    } else {
        Ok(Vec::new())
    }
}

/// Read all provider entries from ~/.zo/settings.json.
pub fn read_providers_file(path: &Path) -> Result<Vec<ProviderEntry>, String> {
    match std::fs::read_to_string(path) {
        Ok(content) => read_providers_from_json_str(&content),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(err) => Err(format!("settings.json 읽기 실패: {err}")),
    }
}

/// Keys a re-save clears when the window no longer writes them. A provider-wide
/// `context_window` now belongs to individual models. `client_fingerprint`
/// must also be removed when a row is saved as a generic client again.
const SUPERSEDED_ROW_KEYS: [&str; 2] = ["context_window", "client_fingerprint"];

/// Re-save a row in place: every key the window writes is replaced where it
/// stands, a superseded key the window no longer writes is cleared, and every
/// other key — the ones a person added for zo (`include_usage`,
/// `supports_reasoning_effort`, `max_output_tokens`, their own `headers`) —
/// stays where it was.
fn resave_row(
    row: &mut serde_json::Map<String, serde_json::Value>,
    written: &serde_json::Map<String, serde_json::Value>,
) {
    for key in SUPERSEDED_ROW_KEYS {
        if !written.contains_key(key) {
            row.shift_remove(key);
        }
    }
    for (key, value) in written {
        row.insert(key.clone(), value.clone());
    }
}

/// Upsert one provider entry into a JSON string, preserving other keys and order.
pub fn upsert_provider_json_str(
    existing_json: &str,
    provider: &ProviderEntry,
) -> Result<String, String> {
    let mut root = settings_root(existing_json)?;

    let new_provider_val = serde_json::to_value(provider).map_err(|err| err.to_string())?;

    let providers_arr = root
        .entry("providers".to_string())
        .or_insert_with(|| serde_json::Value::Array(Vec::new()));

    if let serde_json::Value::Array(arr) = providers_arr {
        let mut replaced = false;
        for item in arr.iter_mut() {
            if let Some(name_str) = item.get("name").and_then(|v| v.as_str())
                && name_str == provider.name
            {
                match (item.as_object_mut(), new_provider_val.as_object()) {
                    (Some(row), Some(written)) => resave_row(row, written),
                    _ => *item = new_provider_val.clone(),
                }
                replaced = true;
                break;
            }
        }
        if !replaced {
            arr.push(new_provider_val);
        }
    } else {
        *providers_arr = serde_json::Value::Array(vec![new_provider_val]);
    }

    render_settings_root(&root)
}

/// Remove one provider entry by name from a JSON string, preserving other keys and order.
pub fn remove_provider_json_str(
    existing_json: &str,
    provider_name: &str,
) -> Result<String, String> {
    if existing_json.trim().is_empty() {
        return Ok(String::new());
    }
    let mut root = settings_root(existing_json)?;

    if let Some(serde_json::Value::Array(arr)) = root.get_mut("providers") {
        arr.retain(|item| item.get("name").and_then(|v| v.as_str()) != Some(provider_name));
    }

    render_settings_root(&root)
}

/// Remove one provider entry from ~/.zo/settings.json file atomically.
pub fn remove_provider_file(path: &Path, provider_name: &str) -> Result<(), String> {
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(format!("파일 읽기 실패: {err}")),
    };
    let updated = remove_provider_json_str(&existing, provider_name)?;
    crate::durable_file::replace_bytes(path, updated.as_bytes())
        .map(|_| ())
        .map_err(|err| format!("파일 쓰기 실패: {err}"))
}

/// The `client_fingerprint` names that mean the Claude Code client — the one
/// identity the window can present, because it reads that client's own install.
/// zo maps the same names to the same client (`client_fingerprint_user_agent`
/// in zo's `openai_compat`); the window execs zo and cannot link it, so the
/// three words are spelled here too. `"claude"` is also the catalogue id and the
/// binary's name on `PATH`, which is where the version is read from.
const CLAUDE_CLIENT_NAMES: [&str; 3] = ["claude", "claude-code", "claude-cli"];
/// The binary whose install names the version the identity presents.
const CLAUDE_CLIENT_BINARY: &str = "claude";
/// Presented when nothing on this machine names a version. Not a pinned
/// number — a pin rots — and a client whitelist reads the shape of the
/// identity, not its version.
const UNKNOWN_CLIENT_VERSION: &str = "0.0.0";

/// `…/share/claude/versions/2.1.268` → `2.1.268`; a target that names no
/// version yields `None`.
fn version_in_symlink_target(target: &str) -> Option<String> {
    let version: String = target
        .rsplit_once("/versions/")?
        .1
        .chars()
        .take_while(|ch| ch.is_ascii_digit() || *ch == '.')
        .collect();
    (!version.is_empty()).then_some(version)
}

/// The `User-Agent` of the client installed on this machine, whose identity a
/// whitelisting gateway accepts.
///
/// Read off the install itself — the catalogue's binary on `PATH` is a symlink
/// into `…/versions/<version>` — because a zo pane derives the same header from
/// that same install. Neither side copies the other's answer, so the identity
/// the connection test passes under is the one the pane presents.
fn installed_client_user_agent() -> String {
    let version = zerocode_core::agent::resolve_on_path(
        std::env::var_os("PATH").as_deref(),
        CLAUDE_CLIENT_BINARY,
    )
    .and_then(|binary| std::fs::read_link(binary).ok())
    .and_then(|target| version_in_symlink_target(&target.to_string_lossy()))
    .unwrap_or_else(|| UNKNOWN_CLIENT_VERSION.to_string());
    format!("claude-cli/{version} (external, cli)")
}

/// The `User-Agent` a connection test presents for a row: the installed Claude
/// client's identity when the row's `client_fingerprint` names that client,
/// nothing otherwise — a row that names no client is probed as the generic
/// client it is, and a row that brings its own `User-Agent` header keeps it.
fn presented_user_agent(
    client_fingerprint: Option<&str>,
    headers: Option<&std::collections::BTreeMap<String, String>>,
) -> Option<String> {
    let row_names_user_agent = headers.is_some_and(|headers| {
        headers
            .keys()
            .any(|name| name.eq_ignore_ascii_case("user-agent"))
    });
    let names_claude = client_fingerprint.is_some_and(|name| {
        CLAUDE_CLIENT_NAMES
            .iter()
            .any(|claude| name.trim().eq_ignore_ascii_case(claude))
    });
    (names_claude && !row_names_user_agent).then(installed_client_user_agent)
}

/// What a gateway said when it refused. An OpenAI-compatible error body names
/// the reason under `error.message`, or as a bare `error` string, or as a
/// top-level `message`. That sentence is the whole difference between a key to
/// fix and a client the gateway will not talk to at all, so the pane shows it
/// instead of a bare status code.
fn refusal_detail(body: &str) -> Option<String> {
    let json: serde_json::Value = serde_json::from_str(body).ok()?;
    let error = json.get("error");
    [
        error.and_then(|error| error.get("message")),
        error,
        json.get("message"),
    ]
    .into_iter()
    .flatten()
    .filter_map(serde_json::Value::as_str)
    .map(str::trim)
    .find(|message| !message.is_empty())
    .map(str::to_string)
}

/// One connection test: the row being connected, as the pane holds it.
pub struct RouterProbe<'a> {
    pub base_url: &'a str,
    pub auth: &'a RouterAuth,
    pub key: Option<&'a str>,
    pub models_path: Option<&'a str>,
    pub id_field: Option<&'a str>,
    pub context_field: Option<&'a str>,
    /// The client the row names (see [`RouterPreset::client_fingerprint`]).
    pub client_fingerprint: Option<&'a str>,
    pub headers: Option<&'a std::collections::BTreeMap<String, String>>,
}

/// Test connection to an OpenAI-compatible endpoint and discover model IDs.
pub async fn test_router_endpoint(probe: &RouterProbe<'_>) -> Result<Vec<DiscoveredModel>, String> {
    let RouterProbe {
        base_url,
        auth,
        key,
        models_path,
        id_field,
        context_field,
        client_fingerprint,
        headers,
    } = *probe;
    let path = models_path.unwrap_or("/models");
    let trimmed_base = base_url.trim_end_matches('/');
    let clean_path = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    let url = format!("{trimmed_base}{clean_path}");

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|err| err.to_string())?;

    let mut req = client.get(&url);

    // The probe presents the client the row names, the same one zo will
    // present from that row's `providers[]` entry — and nothing for a row that
    // names none.
    if let Some(user_agent) = presented_user_agent(client_fingerprint, headers) {
        req = req.header("User-Agent", user_agent);
    }

    if let Some(extra_headers) = headers {
        for (name, value) in extra_headers {
            req = req.header(name, value);
        }
    }

    if let Some(k) = key.filter(|s| !s.trim().is_empty()) {
        let auth_val = if let Some(ref scheme) = auth.scheme {
            format!("{scheme} {}", k.trim())
        } else {
            k.trim().to_string()
        };
        req = req.header(&auth.header, auth_val);
    }

    let res = req
        .send()
        .await
        .map_err(|err| format!("연결 실패: {err}"))?;
    if !res.status().is_success() {
        let status = res.status();
        let body = res.text().await.unwrap_or_default();
        let detail = refusal_detail(&body).unwrap_or_else(|| body.trim().to_string());
        return Err(if detail.is_empty() {
            format!("서버 오류: HTTP {status}")
        } else {
            format!("서버 오류: HTTP {status} — {detail}")
        });
    }

    let body: serde_json::Value = res
        .json()
        .await
        .map_err(|err| format!("응답 파싱 실패: {err}"))?;
    let data_arr = body
        .get("data")
        .and_then(|v| v.as_array())
        .ok_or_else(|| "응답에 data 배열이 없습니다".to_string())?;

    let id_key = id_field.unwrap_or("id");
    let ctx_key = context_field.unwrap_or("context_length");

    let mut models = Vec::new();
    for item in data_arr {
        if let Some(id_val) = item.get(id_key).and_then(|v| v.as_str()) {
            let ctx_len = item.get(ctx_key).and_then(|v| v.as_u64());
            models.push(DiscoveredModel {
                id: id_val.to_string(),
                context_length: ctx_len,
            });
        }
    }

    Ok(models)
}

/// A map standing in for the keychain: every key rule of the window's settings
/// is proven without reaching a real one.
#[cfg(test)]
#[derive(Default)]
pub(crate) struct HeldKeys(std::cell::RefCell<std::collections::BTreeMap<String, String>>);

#[cfg(test)]
impl HeldKeys {
    pub(crate) fn held(&self, service: &str) -> Option<String> {
        self.0.borrow().get(service).cloned()
    }
}

#[cfg(test)]
impl RouterKeys for HeldKeys {
    fn read(&self, service: &str) -> Result<Option<String>, RouterRefusal> {
        Ok(self.held(service))
    }
    fn write(&self, service: &str, secret: &str) -> Result<(), RouterRefusal> {
        self.0
            .borrow_mut()
            .insert(service.to_string(), secret.to_string());
        Ok(())
    }
    fn delete(&self, service: &str) -> Result<(), RouterRefusal> {
        self.0.borrow_mut().remove(service);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn presets_parse_successfully() {
        let presets = load_presets().expect("load presets");
        assert!(!presets.is_empty(), "presets should not be empty");
        let openrouter = presets.iter().find(|p| p.id == "openrouter");
        assert!(openrouter.is_some(), "openrouter preset should exist");
        let or = openrouter.unwrap();
        assert_eq!(or.name, "OpenRouter");
        assert_eq!(or.base_url, "https://openrouter.ai/api/v1");
        assert_eq!(or.auth.header, "Authorization");
        assert_eq!(or.auth.scheme, Some("Bearer".to_string()));
        assert_eq!(or.models.path, "/models");
        assert_eq!(or.models.id_field, "id");

        let agentrouter = presets.iter().find(|p| p.id == "agentrouter");
        assert!(agentrouter.is_some(), "agentrouter preset should exist");
        let ar = agentrouter.unwrap();
        assert_eq!(ar.name, "AgentRouter");
        // Only the gateway that refuses an unknown client carries an identity;
        // every other row is probed and written as the generic client it is.
        assert_eq!(ar.client_fingerprint.as_deref(), Some("claude"));
        for preset in presets.iter().filter(|preset| preset.id != "agentrouter") {
            assert_eq!(
                preset.client_fingerprint, None,
                "{} names a client",
                preset.id
            );
        }
        assert!(presets.iter().all(|preset| preset.headers.is_none()));
    }

    /// A row that names the Claude client presents that client's identity, read
    /// off this machine's install — no version is written down here. A row that
    /// names none presents none: AgentRouter answers an unknown wire image with a
    /// 401 before it ever weighs the key, but OpenRouter and a custom endpoint
    /// have no business being told zo is some other client.
    #[test]
    fn the_presented_identity_comes_from_the_install() {
        assert_eq!(
            version_in_symlink_target("/Users/x/.local/share/claude/versions/2.1.268").as_deref(),
            Some("2.1.268")
        );
        assert_eq!(
            version_in_symlink_target("/opt/claude/versions/2.1.268-rc1").as_deref(),
            Some("2.1.268")
        );
        assert_eq!(
            version_in_symlink_target("/usr/local/bin/claude-wrapper"),
            None
        );
        assert_eq!(
            version_in_symlink_target("/opt/claude/versions/nightly"),
            None
        );

        // Whole identity either way: an install names its version, a machine
        // without one still presents the shape the whitelist reads.
        let presented = installed_client_user_agent();
        assert!(
            presented.starts_with("claude-cli/") && presented.ends_with(" (external, cli)"),
            "{presented}"
        );
        // The identity rides a row that names the Claude client, and only it.
        assert!(presented_user_agent(Some("claude"), None).is_some());
        assert!(presented_user_agent(Some(" Claude-Code "), None).is_some());
        assert_eq!(
            presented_user_agent(None, None),
            None,
            "a generic row presents no client"
        );
        assert_eq!(
            presented_user_agent(Some("codex"), None),
            None,
            "the window cannot present a client it does not read the install of"
        );
        let own =
            std::collections::BTreeMap::from([("User-Agent".to_string(), "mine/1".to_string())]);
        assert_eq!(
            presented_user_agent(Some("claude"), Some(&own)),
            None,
            "a row's own User-Agent wins"
        );
    }

    /// A refusal is shown as what the gateway actually said. The two 401s a
    /// router answers with are not the same failure — one is a key to fix, the
    /// other a client the gateway will not talk to at all — and "HTTP 401"
    /// alone cannot tell them apart.
    #[test]
    fn a_refusal_carries_what_the_gateway_said() {
        // AgentRouter, verbatim: the client wire image was refused before the
        // key was ever weighed.
        assert_eq!(
            refusal_detail(
                r#"{"error":{"message":"unauthorized client detected, contact support"},
                    "message":"UNAUTHENTICATED","success":false,
                    "type":"unauthorized_client_error"}"#
            )
            .as_deref(),
            Some("unauthorized client detected, contact support")
        );
        // The same gateway once the identity passes: now it is about the key.
        assert_eq!(
            refusal_detail(r#"{"error":{"message":"무효한 토큰","type":"new_api_error"}}"#)
                .as_deref(),
            Some("무효한 토큰")
        );
        // Endpoints that name the reason some other way.
        assert_eq!(
            refusal_detail(r#"{"error":"invalid_api_key"}"#).as_deref(),
            Some("invalid_api_key")
        );
        assert_eq!(
            refusal_detail(r#"{"message":"forbidden"}"#).as_deref(),
            Some("forbidden")
        );
        // Nothing to quote: the caller falls back to the raw body or the status.
        assert_eq!(refusal_detail("<html>502</html>"), None);
        assert_eq!(refusal_detail(r#"{"error":{"code":401}}"#), None);
    }

    /// The window writes where zo reads: `ZO_CONFIG_HOME`, then `ZO_HOME`,
    /// then `~/.zo` — zo's own chain (runtime::config). A pane launched with
    /// `ZO_HOME` set would otherwise never see the provider the pane wrote.
    #[test]
    fn the_settings_path_follows_zos_config_home_chain() {
        let cfg = Path::new("/cfg");
        let zo_home = Path::new("/zo-home");
        let home = Path::new("/Users/x");
        assert_eq!(
            zo_settings_path_from(Some(cfg), Some(zo_home), Some(home)),
            Some(PathBuf::from("/cfg/settings.json"))
        );
        assert_eq!(
            zo_settings_path_from(None, Some(zo_home), Some(home)),
            Some(PathBuf::from("/zo-home/settings.json"))
        );
        assert_eq!(
            zo_settings_path_from(None, None, Some(home)),
            Some(PathBuf::from("/Users/x/.zo/settings.json"))
        );
        assert_eq!(zo_settings_path_from(None, None, None), None);
    }

    /// A zo launch is handed the window's own router keys and nothing else:
    /// an entry zo's `/connect` wrote keeps its own key road, an entry with no
    /// stored key hands no empty variable, and the key is read under the
    /// service named by the entry's `auth_env` — the same name the save wrote.
    #[test]
    fn a_zo_launch_carries_the_windows_router_keys_and_nothing_else() {
        let entry = |name: &str, auth_env: &str| ProviderEntry {
            name: name.to_string(),
            base_url: "https://gateway.invalid/v1".to_string(),
            models: vec![ProviderModel::Id("vendor/model".to_string())],
            requires_auth: true,
            auth_env: auth_env.to_string(),
            context_window: None,
            client_fingerprint: None,
            headers: None,
        };
        let providers = vec![
            entry("OpenRouter", &router_auth_env_name("openrouter")),
            entry("DeepSeek", "DEEPSEEK_API_KEY"),
            entry("Unkeyed", &router_auth_env_name("unkeyed")),
            entry("OpenRouter again", &router_auth_env_name("openrouter")),
        ];
        let asked = std::cell::RefCell::new(Vec::new());
        let env = router_launch_env(&providers, |service| {
            asked.borrow_mut().push(service.to_string());
            (service == router_keychain_service_for_env(&router_auth_env_name("openrouter")))
                .then(|| " sk-or-test \n".to_string())
        });
        assert_eq!(
            env,
            vec![(
                "ZEROCODE_ROUTER_OPENROUTER_KEY".to_string(),
                "sk-or-test".to_string()
            )]
        );
        assert!(
            !asked
                .borrow()
                .iter()
                .any(|service| service.contains("DEEPSEEK")),
            "the window asked its keychain for a key zo's /connect owns: {:?}",
            asked.borrow()
        );
    }

    #[test]
    fn naming_helpers_generate_expected_names() {
        assert_eq!(
            router_keychain_service_for_env(&router_auth_env_name("openrouter")),
            "dev.zerocode.router.ZEROCODE_ROUTER_OPENROUTER_KEY"
        );
        assert_eq!(
            router_auth_env_name("agentrouter"),
            "ZEROCODE_ROUTER_AGENTROUTER_KEY"
        );
        assert_eq!(
            router_auth_env_name("custom-endpoint"),
            "ZEROCODE_ROUTER_CUSTOM_ENDPOINT_KEY"
        );
        // A run of other characters is one separator, none at either end.
        assert_eq!(
            router_auth_env_name(" OpenRouter 회사 "),
            "ZEROCODE_ROUTER_OPENROUTER_KEY"
        );
        // A name that spells no ASCII word still names a variable.
        assert_eq!(
            router_auth_env_name("사내 게이트웨이"),
            "ZEROCODE_ROUTER_CUSTOM_KEY"
        );
    }

    fn router_save(name: &str, preset_id: Option<&str>, key: Option<&str>) -> RouterSave {
        RouterSave {
            preset_id: preset_id.map(str::to_string),
            name: name.to_string(),
            base_url: format!(
                "https://{}.invalid/v1",
                router_env_word(name).to_lowercase()
            ),
            key: key.map(str::to_string),
            models: vec![ProviderModel::Id("vendor/model".to_string())],
            client_fingerprint: None,
            headers: None,
        }
    }

    #[test]
    fn invalid_settings_refuse_before_changing_router_keys() {
        for original in ["[]", "null", r#"{"providers": {"unexpected":"data"}}"#] {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("settings.json");
            std::fs::write(&path, original).unwrap();
            let keys = HeldKeys::default();
            let service = router_keychain_service_for_env(&router_auth_env_name("gateway"));
            keys.write(&service, "old-key").unwrap();
            assert!(
                save_router(&path, router_save("gateway", None, Some("new-key")), &keys).is_err()
            );
            assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
            assert_eq!(keys.held(&service).as_deref(), Some("old-key"));
        }
    }

    #[test]
    fn resaving_as_a_generic_client_clears_the_previous_fingerprint() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let keys = HeldKeys::default();
        let mut branded = router_save("gateway", None, None);
        branded.client_fingerprint = Some("claude".into());
        save_router(&path, branded, &keys).unwrap();
        let rows = save_router(&path, router_save("gateway", None, None), &keys).unwrap();
        assert_eq!(rows[0].client_fingerprint, None);
    }

    #[test]
    fn a_failed_settings_write_restores_the_previous_key_or_removes_the_new_one() {
        for previous in [None, Some("old-key")] {
            let dir = tempfile::tempdir().expect("tempdir");
            let path = dir.path().join("settings.json");
            let keys = HeldKeys::default();
            save_router(&path, router_save("gateway", None, previous), &keys).unwrap();
            let existing = std::fs::read_to_string(&path).unwrap();
            let service = router_keychain_service_for_env(&router_auth_env_name("gateway"));
            let refused = save_router_with_writer(
                &path,
                router_save("gateway", None, Some("new-key")),
                &keys,
                |_, _| Err("disk refused the write".into()),
            )
            .unwrap_err();
            assert!(refused.message.contains("disk refused"));
            assert_eq!(keys.held(&service).as_deref(), previous);
            assert_eq!(std::fs::read_to_string(&path).unwrap(), existing);
        }
    }

    #[test]
    fn a_failed_key_deletion_keeps_the_configured_router() {
        struct RefusesDeletion(HeldKeys);
        impl RouterKeys for RefusesDeletion {
            fn read(&self, service: &str) -> Result<Option<String>, RouterRefusal> {
                self.0.read(service)
            }
            fn write(&self, service: &str, secret: &str) -> Result<(), RouterRefusal> {
                self.0.write(service, secret)
            }
            fn delete(&self, _: &str) -> Result<(), RouterRefusal> {
                Err("keychain locked".to_string().into())
            }
        }
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let keys = RefusesDeletion(HeldKeys::default());
        save_router(&path, router_save("gateway", None, Some("key")), &keys).unwrap();
        let original = std::fs::read_to_string(&path).unwrap();
        assert!(
            remove_router(&path, "gateway", &keys)
                .unwrap_err()
                .contains("keychain locked")
        );
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
    }

    /// Rows an older window wrote before rows owned their variables can share
    /// one (two Hangul names both spelled `ZEROCODE_ROUTER_________KEY`). A
    /// save that brings a key moves its row to a variable of its own, so the
    /// other row is no longer handed this key; a keyless save keeps the shared
    /// variable rather than lose the only key the row can reach.
    #[test]
    fn a_row_sharing_an_older_windows_variable_moves_out_when_its_key_is_given() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let shared = "ZEROCODE_ROUTER_________KEY";
        std::fs::write(
            &path,
            serde_json::json!({ "providers": [
                { "name": "사내 게이트웨이", "base_url": "https://gw.corp.invalid/v1",
                  "models": ["vendor/model"], "requires_auth": true, "auth_env": shared },
                { "name": "개인 게이트웨이", "base_url": "https://my.invalid/v1",
                  "models": ["vendor/model"], "requires_auth": true, "auth_env": shared },
            ]})
            .to_string(),
        )
        .expect("legacy rows");
        let keys = HeldKeys::default();
        keys.write(&router_keychain_service_for_env(shared), "sk-last-written")
            .expect("legacy item");

        save_router(&path, router_save("개인 게이트웨이", None, None), &keys).expect("keyless");
        let rows = read_providers_file(&path).expect("rows");
        assert_eq!(
            rows[1].auth_env, shared,
            "no key given: the row keeps what it could reach"
        );

        save_router(
            &path,
            router_save("사내 게이트웨이", None, Some("sk-corp")),
            &keys,
        )
        .expect("re-save with its key");
        let rows = read_providers_file(&path).expect("rows");
        assert_ne!(rows[0].auth_env, shared, "the row with its key moved out");
        assert_eq!(rows[1].auth_env, shared);
        let launch = router_launch_env(&rows, |service| keys.held(service));
        assert!(
            launch.contains(&(rows[0].auth_env.clone(), "sk-corp".to_string())),
            "{launch:?}"
        );
        assert!(
            !launch
                .iter()
                .any(|(env, key)| env == shared && key == "sk-corp"),
            "the other row is not handed this key"
        );
    }

    /// The row identity is the backend's and it is never shared: two Hangul
    /// names that spell no ASCII word, and a second row saved from the same
    /// preset, each get a variable of their own, so each gateway is handed its
    /// own key. A re-save of a row keeps the variable its first save chose.
    #[test]
    fn every_row_reads_its_own_key() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let keys = HeldKeys::default();
        let saved = [
            ("사내 게이트웨이", None, "sk-corp"),
            ("개인 게이트웨이", None, "sk-mine"),
            ("OpenRouter", Some("openrouter"), "sk-or-1"),
            ("OpenRouter 회사", Some("openrouter"), "sk-or-2"),
        ];
        for (name, preset, key) in saved {
            save_router(&path, router_save(name, preset, Some(key)), &keys).expect("save");
        }
        let rows = read_providers_file(&path).expect("rows");
        let envs: Vec<&str> = rows.iter().map(|row| row.auth_env.as_str()).collect();
        assert_eq!(
            envs,
            [
                "ZEROCODE_ROUTER_CUSTOM_KEY",
                "ZEROCODE_ROUTER_CUSTOM_2_KEY",
                "ZEROCODE_ROUTER_OPENROUTER_KEY",
                "ZEROCODE_ROUTER_OPENROUTER_2_KEY",
            ]
        );
        // What a zo launch is handed: one variable per row, each with its key.
        let launch = router_launch_env(&rows, |service| keys.held(service));
        let handed: Vec<(&str, &str)> = launch
            .iter()
            .map(|(env, key)| (env.as_str(), key.as_str()))
            .collect();
        assert_eq!(
            handed,
            envs.iter()
                .copied()
                .zip(saved.iter().map(|(_, _, key)| *key))
                .collect::<Vec<_>>()
        );

        // A re-save keeps the row's variable and touches no other row's key.
        save_router(
            &path,
            router_save("개인 게이트웨이", None, Some("sk-mine-2")),
            &keys,
        )
        .expect("re-save");
        let rows = read_providers_file(&path).expect("rows");
        assert_eq!(rows[1].auth_env, "ZEROCODE_ROUTER_CUSTOM_2_KEY");
        assert_eq!(
            keys.held(&router_keychain_service_for_env(
                "ZEROCODE_ROUTER_CUSTOM_KEY"
            ))
            .as_deref(),
            Some("sk-corp")
        );
        assert_eq!(
            keys.held(&router_keychain_service_for_env(
                "ZEROCODE_ROUTER_CUSTOM_2_KEY"
            ))
            .as_deref(),
            Some("sk-mine-2")
        );
    }

    /// `requires_auth` says whether the row has a key: one given now, or one
    /// its variable already holds. A keyless Ollama (or an OpenRouter saved
    /// without a key) is written keyless, so zo builds it instead of refusing
    /// it for a key nobody was ever going to give.
    #[test]
    fn a_row_requires_a_key_only_when_it_has_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let keys = HeldKeys::default();
        let requires = |name: &str| {
            read_providers_file(&path)
                .expect("rows")
                .into_iter()
                .find(|row| row.name == name)
                .map(|row| row.requires_auth)
        };

        save_router(&path, router_save("Ollama", Some("ollama"), None), &keys).expect("save");
        assert_eq!(
            requires("Ollama"),
            Some(false),
            "a keyless row demands a key"
        );

        save_router(
            &path,
            router_save("OpenRouter", Some("openrouter"), Some("sk-or")),
            &keys,
        )
        .expect("save");
        assert_eq!(requires("OpenRouter"), Some(true));
        // A re-save that leaves the key box empty keeps the stored key.
        save_router(
            &path,
            router_save("OpenRouter", Some("openrouter"), None),
            &keys,
        )
        .expect("re-save");
        assert_eq!(
            requires("OpenRouter"),
            Some(true),
            "the stored key was forgotten"
        );

        // A new row whose variable still holds an item no row reads (a row
        // removed before removals found their own key) is not handed that
        // stranger's key: it is keyless, and the stale item is gone.
        let stale = router_keychain_service_for_env("ZEROCODE_ROUTER_AGENTROUTER_KEY");
        keys.write(&stale, "sk-stale").expect("seed");
        save_router(
            &path,
            router_save("AgentRouter", Some("agentrouter"), None),
            &keys,
        )
        .expect("save");
        assert_eq!(requires("AgentRouter"), Some(false));
        assert_eq!(keys.held(&stale), None, "a new row inherited a stale key");
    }

    /// Off macOS there is no keychain to keep a router key in. A save that
    /// brings one fails with a kind the pane translates, and writes no row —
    /// it used to report success and keep nothing, so every zo request went
    /// out keyless. The same row saved without a key is still a keyless row.
    #[test]
    fn a_machine_without_a_keychain_refuses_the_key_and_says_why() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let nowhere = Keychain::without_a_store();
        let refused = save_router(
            &path,
            router_save("AgentRouter", Some("agentrouter"), Some("sk-ar")),
            &nowhere,
        )
        .expect_err("a key with nowhere to live was accepted");
        assert_eq!(refused.kind, RouterRefusalKind::KeychainUnavailable);
        assert!(
            read_providers_file(&path).expect("rows").is_empty(),
            "a refused save wrote its row"
        );

        let rows = save_router(&path, router_save("Ollama", Some("ollama"), None), &nowhere)
            .expect("a keyless save needs no keychain");
        assert_eq!(rows.len(), 1);
        assert!(!rows[0].requires_auth);
        // The pane is told the same thing the save acts on, before anyone
        // types a key; this machine's own keychain answers for this platform.
        assert!(!nowhere.keeps_keys());
        assert_eq!(
            Keychain::of_this_machine().keeps_keys(),
            crate::accounts::KEYCHAIN_AVAILABLE
        );
    }

    /// The pane words its hint and offers its key field by what the window
    /// answers; an unregistered command would leave it promising the macOS
    /// Keychain on every platform again.
    #[test]
    fn the_pane_asks_the_window_where_a_key_would_go() {
        let main = include_str!("main.rs");
        let handlers = main
            .split_once("tauri::generate_handler![")
            .map(|(_, list)| list)
            .expect("the handler list");
        assert!(
            handlers.contains("api_router_keys_kept,"),
            "the key-store command is not registered"
        );
        let pane = include_str!("../../../ui/shell-settings.js");
        assert!(pane.contains("invoke(\"api_router_keys_kept\")"));
        let page = include_str!("../../../ui/index.html");
        for hint in [
            "id=\"router-keychain-hint\"",
            "id=\"router-no-keychain-hint\"",
        ] {
            assert!(page.contains(hint), "the pane lost {hint}");
        }
    }

    /// A removal deletes the key named by the row's own stored variable — not
    /// one guessed from its display name — and only when no other row reads it.
    #[test]
    fn removing_a_row_deletes_its_own_key_and_no_other() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let keys = HeldKeys::default();
        // Renamed at its save: the variable is spelled from the preset.
        save_router(
            &path,
            router_save("OpenRouter 회사", Some("openrouter"), Some("sk-or")),
            &keys,
        )
        .expect("save");
        let service = router_keychain_service_for_env("ZEROCODE_ROUTER_OPENROUTER_KEY");
        assert_eq!(keys.held(&service).as_deref(), Some("sk-or"));
        let rows = remove_router(&path, "OpenRouter 회사", &keys).expect("remove");
        assert!(rows.is_empty(), "{rows:?}");
        assert_eq!(
            keys.held(&service),
            None,
            "the renamed row's key outlived it"
        );

        // Two rows a hand (or an older window) left on one variable: removing
        // one leaves the key the other still reads.
        std::fs::write(
            &path,
            r#"{"providers":[
                {"name":"A","base_url":"https://a.invalid/v1","models":[],"requires_auth":true,"auth_env":"ZEROCODE_ROUTER_SHARED_KEY"},
                {"name":"B","base_url":"https://b.invalid/v1","models":[],"requires_auth":true,"auth_env":"ZEROCODE_ROUTER_SHARED_KEY"}
            ]}"#,
        )
        .expect("seed");
        let shared = router_keychain_service_for_env("ZEROCODE_ROUTER_SHARED_KEY");
        keys.write(&shared, "sk-shared").expect("seed key");
        remove_router(&path, "A", &keys).expect("remove A");
        assert_eq!(
            keys.held(&shared).as_deref(),
            Some("sk-shared"),
            "B lost its key"
        );
        remove_router(&path, "B", &keys).expect("remove B");
        assert_eq!(
            keys.held(&shared),
            None,
            "the last reader's key outlived it"
        );
    }

    #[test]
    fn settings_upsert_keeps_other_keys_and_order() {
        // A file with other keys keeps them; preserving insertion order.
        let initial_json = r#"{
  "theme": "dark",
  "locale": "ko",
  "model": "gemini-flash"
}
"#;
        let provider = ProviderEntry {
            name: "OpenRouter".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            models: vec![ProviderModel::Id("anthropic/claude-3.7-sonnet".to_string())],
            requires_auth: true,
            auth_env: "ZEROCODE_ROUTER_OPENROUTER_KEY".to_string(),
            context_window: Some(200000),
            client_fingerprint: None,
            headers: None,
        };

        let result = upsert_provider_json_str(initial_json, &provider).expect("upsert");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("valid json");

        assert_eq!(parsed["theme"], "dark");
        assert_eq!(parsed["locale"], "ko");
        assert_eq!(parsed["model"], "gemini-flash");
        assert_eq!(parsed["providers"][0]["name"], "OpenRouter");
        assert_eq!(
            parsed["providers"][0]["models"][0],
            "anthropic/claude-3.7-sonnet"
        );
        assert_eq!(parsed["providers"][0]["context_window"], 200000);

        // Verify key ordering: theme, locale, model, providers
        let keys: Vec<&str> = parsed
            .as_object()
            .unwrap()
            .keys()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(keys, vec!["theme", "locale", "model", "providers"]);
    }

    #[test]
    fn settings_upsert_overwrites_same_name() {
        // The same name overwrites existing entry in place
        let initial_json = r#"{
  "providers": [
    {
      "name": "OpenRouter",
      "base_url": "https://openrouter.ai/api/v1",
      "models": ["model-a"],
      "requires_auth": true,
      "auth_env": "ZEROCODE_ROUTER_OPENROUTER_KEY"
    }
  ]
}
"#;
        let updated_provider = ProviderEntry {
            name: "OpenRouter".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            models: ["model-a", "model-b", "model-c"]
                .map(|id| ProviderModel::Id(id.to_string()))
                .to_vec(),
            requires_auth: true,
            auth_env: "ZEROCODE_ROUTER_OPENROUTER_KEY".to_string(),
            context_window: Some(128000),
            client_fingerprint: None,
            headers: None,
        };

        let result = upsert_provider_json_str(initial_json, &updated_provider).expect("upsert");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("valid json");

        let providers = parsed["providers"].as_array().expect("providers array");
        assert_eq!(
            providers.len(),
            1,
            "same name must overwrite, not duplicate"
        );
        assert_eq!(providers[0]["name"], "OpenRouter");
        assert_eq!(providers[0]["models"].as_array().unwrap().len(), 3);
        assert_eq!(providers[0]["context_window"], 128000);
    }

    /// A re-save rewrites what the window decides and nothing else. The keys
    /// a person added to the row for zo — `include_usage` for a gateway that
    /// refuses `stream_options`, `supports_reasoning_effort`, a
    /// `max_output_tokens`, their own `headers` — stay where they were. The
    /// provider-wide `context_window` is the one key the window clears: it now
    /// states each model's own, and the one an older window wrote was a single
    /// model's number.
    #[test]
    fn a_resave_keeps_what_a_person_added_for_zo() {
        let initial_json = r#"{
  "providers": [
    {
      "name": "OpenRouter",
      "base_url": "https://openrouter.ai/api/v1",
      "include_usage": false,
      "models": ["model-a"],
      "requires_auth": true,
      "auth_env": "ZEROCODE_ROUTER_OPENROUTER_KEY",
      "context_window": 8192,
      "supports_reasoning_effort": true,
      "max_output_tokens": 16000,
      "headers": {"HTTP-Referer": "https://example.invalid"}
    }
  ]
}
"#;
        let resaved = ProviderEntry {
            name: "OpenRouter".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            models: vec![ProviderModel::Id("model-b".to_string())],
            requires_auth: true,
            auth_env: "ZEROCODE_ROUTER_OPENROUTER_KEY".to_string(),
            context_window: None,
            client_fingerprint: None,
            headers: None,
        };
        let result = upsert_provider_json_str(initial_json, &resaved).expect("upsert");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("valid json");
        let row = &parsed["providers"][0];
        assert_eq!(row["models"], serde_json::json!(["model-b"]));
        assert_eq!(row["include_usage"], false, "{row}");
        assert_eq!(row["supports_reasoning_effort"], true, "{row}");
        assert_eq!(row["max_output_tokens"], 16000, "{row}");
        assert_eq!(
            row["headers"],
            serde_json::json!({"HTTP-Referer": "https://example.invalid"}),
            "{row}"
        );
        assert!(row.get("context_window").is_none(), "{row}");
        // Where they were: the row reads the same way it did.
        let keys: Vec<&str> = row
            .as_object()
            .expect("row")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            [
                "name",
                "base_url",
                "include_usage",
                "models",
                "requires_auth",
                "auth_env",
                "supports_reasoning_effort",
                "max_output_tokens",
                "headers"
            ]
        );
    }

    #[test]
    fn settings_upsert_adds_second_name() {
        // A second name adds a new entry
        let initial_json = r#"{
  "providers": [
    {
      "name": "OpenRouter",
      "base_url": "https://openrouter.ai/api/v1",
      "models": ["model-a"],
      "requires_auth": true,
      "auth_env": "ZEROCODE_ROUTER_OPENROUTER_KEY"
    }
  ]
}
"#;
        let second_provider = ProviderEntry {
            name: "AgentRouter".to_string(),
            base_url: "https://agentrouter.org/v1".to_string(),
            models: vec![ProviderModel::Id("agent-1".to_string())],
            requires_auth: true,
            auth_env: "ZEROCODE_ROUTER_AGENTROUTER_KEY".to_string(),
            context_window: None,
            client_fingerprint: Some("claude-code".to_string()),
            headers: None,
        };

        let result = upsert_provider_json_str(initial_json, &second_provider).expect("upsert");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("valid json");

        let providers = parsed["providers"].as_array().expect("providers array");
        assert_eq!(providers.len(), 2, "second name must add to providers list");
        assert_eq!(providers[0]["name"], "OpenRouter");
        assert_eq!(providers[1]["name"], "AgentRouter");
        assert_eq!(providers[1]["base_url"], "https://agentrouter.org/v1");
        assert_eq!(providers[1]["client_fingerprint"], "claude-code");
    }

    /// A row's models carry each model's own context window, the one the
    /// connection test read for that model — in the shape zo parses beside a
    /// plain id (`CustomModel`). A row written that way still reads back into
    /// the pane's list, and no provider-wide window speaks for every model.
    #[test]
    fn a_row_with_per_model_windows_reads_back_into_the_list() {
        let written = r#"{"providers":[{"name":"OpenRouter",
            "base_url":"https://openrouter.ai/api/v1","requires_auth":true,
            "auth_env":"ZEROCODE_ROUTER_OPENROUTER_KEY",
            "models":[{"id":"vendor/big","context_window":1048576},"vendor/plain"]}]}"#;
        let rows = read_providers_from_json_str(written).expect("rows");
        assert_eq!(rows.len(), 1, "a row with per-model windows left the list");
        assert_eq!(rows[0].models.len(), 2);
    }

    #[test]
    fn a_row_states_each_models_own_window() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let mut save = router_save("OpenRouter", Some("openrouter"), None);
        // As the pane sends them: each checked model with the window the probe
        // read for it, `null` where the gateway said nothing.
        save.models = serde_json::from_value(serde_json::json!([
            {"id": "vendor/big", "context_window": 1_048_576},
            {"id": "vendor/small", "context_window": 32_768},
            {"id": "vendor/plain", "context_window": null},
        ]))
        .expect("the pane's shape");
        save_router(&path, save, &HeldKeys::default()).expect("save");
        let file: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("file")).expect("json");
        let row = &file["providers"][0];
        assert_eq!(
            row["models"],
            serde_json::json!([
                {"id": "vendor/big", "context_window": 1_048_576},
                {"id": "vendor/small", "context_window": 32_768},
                "vendor/plain"
            ])
        );
        assert!(
            row.get("context_window").is_none(),
            "one model's window was written for every model: {row}"
        );
    }

    #[test]
    fn settings_remove_provider_deletes_specified_entry() {
        let initial_json = r#"{
  "providers": [
    {
      "name": "OpenRouter",
      "base_url": "https://openrouter.ai/api/v1",
      "models": ["model-a"],
      "requires_auth": true,
      "auth_env": "ZEROCODE_ROUTER_OPENROUTER_KEY"
    },
    {
      "name": "AgentRouter",
      "base_url": "https://agentrouter.org/v1",
      "models": ["agent-1"],
      "requires_auth": true,
      "auth_env": "ZEROCODE_ROUTER_AGENTROUTER_KEY"
    }
  ]
}
"#;
        let result = remove_provider_json_str(initial_json, "OpenRouter").expect("remove");
        let parsed: serde_json::Value = serde_json::from_str(&result).expect("valid json");
        let providers = parsed["providers"].as_array().expect("providers array");
        assert_eq!(providers.len(), 1);
        assert_eq!(providers[0]["name"], "AgentRouter");
    }

    #[test]
    fn keychain_write_uses_service_and_stores_key() {
        let service = router_keychain_service_for_env(&router_auth_env_name("openrouter"));
        let secret = "sk-or-v1-secret-token";
        crate::accounts::write_keychain_service(&service, secret).expect("write keychain");
        #[cfg(target_os = "macos")]
        {
            let read_back =
                crate::accounts::read_keychain_service(&service).expect("read keychain");
            assert_eq!(read_back, secret);
        }
    }
}
