//! Saved model connections and explicit role assignments over the existing provider table.

use std::path::Path;

use judgment_provider::connections::{self, Reference};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::api_routers::{self, ProviderEntry, RouterKeys};

const JUDGMENT_KEYS: [&str; 3] = [
    judgment_provider::SETTINGS_KEY,
    "jevModel",
    local_judgment::MANAGED_MODEL_KEY,
];

/// The format each saved connection was last applied to judgment in, by its
/// id (t-76211), kept in zo's `smart` beside the role keys. Not a part of
/// the judgment selection: clearing judgment leaves it, so a connection
/// picked again is asked in its own format after a restart, not in OpenAI
/// JSON. Only the window reads it.
const JUDGMENT_FORMATS_KEY: &str = "judgmentFormats";

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Coding,
    Judgment,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionRow {
    pub id: String,
    pub generation: u64,
    pub provider: ProviderEntry,
    pub auth: connections::Authentication,
    pub probe_headers: Option<std::collections::BTreeMap<String, String>>,
    pub key_available: bool,
    /// Whether the old judgment setting kept a key for this row's address
    /// that the row itself cannot reach — what the pane offers to move.
    pub legacy_key: bool,
    pub uses: Vec<Role>,
    /// The format judgment last used this connection in (t-76211): the one
    /// applied last, else the judgment reference's own when it names this
    /// row; `None` for one never applied to judgment.
    pub judgment_format: Option<judgment_provider::Protocol>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub connections: Vec<ConnectionRow>,
    pub coding: Option<Reference>,
    pub judgment: Option<Reference>,
    pub coding_model: Option<String>,
    pub active_provider_identity: String,
    pub legacy_key_present: bool,
}

fn root(path: &Path) -> Result<Value, String> {
    api_routers::read_zo_settings_root(path).map(Value::Object)
}

fn reference(root: &Value, key: &str) -> Result<Option<Reference>, String> {
    root.get("smart")
        .and_then(|smart| smart.get(key))
        .map(|value| {
            serde_json::from_value(value.clone()).map_err(|_| "provider_settings".to_string())
        })
        .transpose()
}

/// The format `id` was last applied to judgment in, as [`JUDGMENT_FORMATS_KEY`]
/// keeps it; a word this build does not know is no format.
fn remembered_format(root: &Value, id: &str) -> Option<judgment_provider::Protocol> {
    let held = root.get("smart")?.get(JUDGMENT_FORMATS_KEY)?.get(id)?;
    serde_json::from_value(held.clone()).ok()
}

/// Keep each of `references`' formats as its connection's, the last word
/// for a connection named twice, and only the formats of connections still
/// saved (`ids`): a deleted row's goes with the next apply.
fn remember_formats<'a>(
    smart: &mut Map<String, Value>,
    ids: &[String],
    references: impl IntoIterator<Item = &'a Reference>,
) -> Result<(), String> {
    let formats = smart
        .entry(JUDGMENT_FORMATS_KEY)
        .or_insert_with(|| json!({}));
    if !formats.is_object() {
        *formats = json!({});
    }
    let formats = formats.as_object_mut().ok_or("provider_settings")?;
    formats.retain(|id, _| ids.contains(id));
    for reference in references {
        if ids.contains(&reference.id) {
            formats.insert(
                reference.id.clone(),
                serde_json::to_value(reference.protocol).map_err(|error| error.to_string())?,
            );
        }
    }
    Ok(())
}

fn coding_matches(root: &Value, reference: &Reference) -> bool {
    let Some((name, model)) = root
        .get("model")
        .and_then(Value::as_str)
        .and_then(|model| model.split_once('/'))
    else {
        return false;
    };
    let Ok(row) = connections::row(root, &reference.id) else {
        return false;
    };
    let Ok(metadata) = connections::metadata(row) else {
        return false;
    };
    model == reference.model
        && (row
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|current| connections::names_match(current, name))
            || metadata.id == name
            || metadata
                .aliases
                .iter()
                .any(|alias| connections::names_match(alias, name)))
}

fn coding_reference(root: &Value) -> Result<Option<Reference>, String> {
    if let Ok(Some(mut reference)) = reference(root, connections::CODING_CONNECTION_KEY)
        && coding_matches(root, &reference)
    {
        let row = connections::row(root, &reference.id).map_err(|error| error.to_string())?;
        reference.generation = connections::metadata(row)
            .map_err(|error| error.to_string())?
            .generation;
        return Ok(Some(reference));
    }
    let Some((name, model)) = root
        .get("model")
        .and_then(Value::as_str)
        .and_then(|model| model.split_once('/'))
    else {
        return Ok(None);
    };
    for row in root
        .get("providers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Ok(metadata) = connections::metadata(row) else {
            continue;
        };
        let reference = Reference {
            id: metadata.id,
            generation: metadata.generation,
            model: model.to_string(),
            protocol: judgment_provider::Protocol::OpenaiJson,
        };
        if !name.is_empty() && coding_matches(root, &reference) {
            return Ok(Some(reference));
        }
    }
    Ok(None)
}

pub fn catalog(path: &Path, keys: &dyn RouterKeys) -> Result<Catalog, String> {
    let root = root(path)?;
    let coding = coding_reference(&root)?;
    let judgment = reference(&root, connections::JUDGMENT_CONNECTION_KEY)?;
    let active =
        judgment_provider::Provider::from_root(&root).map_err(|error| error.to_string())?;
    let legacy_key_present = if active.connection.is_some() && active.requires_auth {
        let mut legacy = active.clone();
        legacy.connection = None;
        keys.read(&crate::typesafe_settings::provider_keychain_service(
            &legacy,
        ))
        .map_err(|error| error.message)?
        .is_some_and(|key| !key.trim().is_empty())
    } else {
        false
    };
    let mut connections = Vec::new();
    for row in root
        .get("providers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let mut provider: ProviderEntry =
            serde_json::from_value(row.clone()).map_err(|error| error.to_string())?;
        provider.requires_auth = connections::requires_auth(row);
        let metadata = connections::metadata(row).map_err(|error| error.to_string())?;
        let key_available = !provider.requires_auth
            || api_routers::read_connection_key(&provider.auth_env, keys)
                .map_err(|error| error.message)?
                .is_some_and(|key| !key.trim().is_empty());
        let legacy_key = !key_available && legacy_key_of(&root, &metadata, row, keys)?.is_some();
        let uses = [
            (Role::Coding, coding.as_ref()),
            (Role::Judgment, judgment.as_ref()),
        ]
        .into_iter()
        .filter_map(|(role, reference)| {
            reference
                .filter(|reference| reference.id == metadata.id)
                .map(|_| role)
        })
        .collect();
        let judgment_format = remembered_format(&root, &metadata.id).or_else(|| {
            judgment
                .as_ref()
                .filter(|reference| reference.id == metadata.id)
                .map(|reference| reference.protocol)
        });
        connections.push(ConnectionRow {
            id: metadata.id,
            generation: metadata.generation,
            provider,
            auth: metadata.auth,
            probe_headers: api_routers::probe_headers(row)?,
            key_available,
            legacy_key,
            uses,
            judgment_format,
        });
    }
    Ok(Catalog {
        connections,
        coding,
        judgment,
        coding_model: root
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string),
        active_provider_identity: active.identity(),
        legacy_key_present,
    })
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Check {
    pub preflight: crate::typesafe_settings::ProviderPreflight,
    pub legacy_key_present: bool,
}

pub fn check(
    path: &Path,
    reference: &Reference,
    expected: &str,
    keys: &dyn RouterKeys,
    wire: &crate::systemone::Wire,
) -> Result<Check, String> {
    let candidate =
        connections::candidate(&root(path)?, reference).map_err(|error| error.to_string())?;
    let preflight = crate::typesafe_settings::preflight_provider(
        path,
        candidate.clone(),
        expected,
        keys,
        wire,
    )?;
    let now = connections::candidate(&root(path)?, reference).map_err(|error| error.to_string())?;
    if now.identity() != candidate.identity() {
        return Err("settings_changed".to_string());
    }
    let mut legacy = candidate;
    legacy.connection = None;
    let legacy_key_present = legacy.requires_auth
        && keys
            .read(&crate::typesafe_settings::provider_keychain_service(
                &legacy,
            ))
            .map_err(|error| error.message)?
            .is_some_and(|key| !key.trim().is_empty());
    Ok(Check {
        preflight,
        legacy_key_present,
    })
}

/// Applying performs a fresh preflight through the same door. A stale UI check is
/// never authority to apply; no settings/key lock is held while the request runs.
pub fn apply_judgment(
    path: &Path,
    reference: &Reference,
    expected: &str,
    keys: &dyn RouterKeys,
    wire: &crate::systemone::Wire,
) -> Result<(), String> {
    let checked = check(path, reference, expected, keys, wire)?;
    if !checked.preflight.answered {
        return Err(checked
            .preflight
            .failure
            .unwrap_or_else(|| "schema".to_string()));
    }
    api_routers::update_zo_settings_root(path, |document| {
        let root = Value::Object(document.clone());
        let active =
            judgment_provider::Provider::from_root(&root).map_err(|error| error.to_string())?;
        if active.identity() != expected {
            return Err("settings_changed".to_string());
        }
        let candidate =
            connections::candidate(&root, reference).map_err(|error| error.to_string())?;
        if candidate.identity() != checked.preflight.candidate_identity {
            return Err("settings_changed".to_string());
        }
        persist_identity(document, &reference.id)?;
        let saved: Vec<String> = document
            .get("providers")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|row| connections::metadata(row).ok().map(|metadata| metadata.id))
            .collect();
        let smart = smart(document)?;
        // The connection judgment leaves keeps the format it was used in,
        // even one applied before the window kept formats (t-76211).
        let replaced: Option<Reference> = smart
            .get(connections::JUDGMENT_CONNECTION_KEY)
            .and_then(|held| serde_json::from_value(held.clone()).ok());
        if !smart.contains_key(connections::JUDGMENT_CONNECTION_KEY) {
            let previous: Map<String, Value> = JUDGMENT_KEYS
                .iter()
                .map(|key| {
                    (
                        (*key).to_string(),
                        smart.get(*key).cloned().unwrap_or(Value::Null),
                    )
                })
                .collect();
            smart.insert(
                connections::PREVIOUS_JUDGMENT_KEY.to_string(),
                Value::Object(previous),
            );
        }
        smart.insert(
            judgment_provider::SETTINGS_KEY.to_string(),
            serde_json::to_value(candidate).map_err(|error| error.to_string())?,
        );
        smart.insert(
            connections::JUDGMENT_CONNECTION_KEY.to_string(),
            serde_json::to_value(reference).map_err(|error| error.to_string())?,
        );
        remember_formats(smart, &saved, replaced.iter().chain([reference]))?;
        smart.remove(local_judgment::MANAGED_MODEL_KEY);
        Ok(())
    })
}

pub fn apply_coding(path: &Path, reference: &Reference) -> Result<(), String> {
    api_routers::update_zo_settings_root(path, |document| {
        let root = Value::Object(document.clone());
        let row = connections::row(&root, &reference.id).map_err(|error| error.to_string())?;
        let metadata = connections::metadata(row).map_err(|error| error.to_string())?;
        if metadata.generation != reference.generation
            || reference.model.is_empty()
            || reference.model.len() > 256
            || reference.model.chars().any(char::is_control)
        {
            return Err("settings_changed".to_string());
        }
        let name = row
            .get("name")
            .and_then(Value::as_str)
            .ok_or("provider_settings")?;
        let declared = row
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| {
                model
                    .as_str()
                    .or_else(|| model.get("id").and_then(Value::as_str))
            })
            .find(|model| model.eq_ignore_ascii_case(&reference.model))
            .ok_or("provider_model")?;
        let model = format!("{name}/{declared}");
        persist_identity(document, &reference.id)?;
        let previous = document.get("model").cloned().unwrap_or(Value::Null);
        let owned = self::reference(&root, connections::CODING_CONNECTION_KEY)?
            .is_some_and(|held| coding_matches(&root, &held));
        let smart = smart(document)?;
        if !owned {
            smart.insert(connections::PREVIOUS_CODING_KEY.to_string(), previous);
        }
        smart.insert(
            connections::CODING_CONNECTION_KEY.to_string(),
            serde_json::to_value(reference).map_err(|error| error.to_string())?,
        );
        document.insert("model".to_string(), Value::String(model));
        Ok(())
    })
}

fn persist_identity(document: &mut Map<String, Value>, id: &str) -> Result<(), String> {
    let rows = document
        .get_mut("providers")
        .and_then(Value::as_array_mut)
        .ok_or("provider_settings")?;
    for row in rows {
        let metadata = connections::metadata(row).map_err(|error| error.to_string())?;
        if metadata.id == id {
            row.as_object_mut()
                .ok_or("provider_settings")?
                .insert("connection".to_string(), json!(metadata));
            return Ok(());
        }
    }
    Err("settings_changed".to_string())
}

pub(crate) fn smart(document: &mut Map<String, Value>) -> Result<&mut Map<String, Value>, String> {
    document
        .entry("smart")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| "provider_settings".to_string())
}

pub(crate) fn clear_in(document: &mut Map<String, Value>, role: Role) -> Result<(), String> {
    let root = Value::Object(document.clone());
    let restore_coding = reference(&root, connections::CODING_CONNECTION_KEY)?
        .is_some_and(|held| coding_matches(&root, &held));
    let smart = smart(document)?;
    match role {
        Role::Judgment => {
            if smart.remove(connections::JUDGMENT_CONNECTION_KEY).is_some() {
                let previous = smart
                    .remove(connections::PREVIOUS_JUDGMENT_KEY)
                    .ok_or("provider_settings")?;
                let previous = previous.as_object().ok_or("provider_settings")?;
                for key in JUDGMENT_KEYS {
                    match previous.get(key).filter(|value| !value.is_null()) {
                        Some(value) => {
                            smart.insert(key.to_string(), value.clone());
                        }
                        None => {
                            smart.remove(key);
                        }
                    }
                }
            }
        }
        Role::Coding => {
            smart.remove(connections::CODING_CONNECTION_KEY);
            let previous = smart
                .remove(connections::PREVIOUS_CODING_KEY)
                .filter(|_| restore_coding)
                .unwrap_or(Value::Null);
            if previous.is_null() {
                document.remove("model");
            } else {
                document.insert("model".to_string(), previous);
            }
        }
    }
    Ok(())
}

pub fn clear(path: &Path, role: Role, expected: Option<&Reference>) -> Result<(), String> {
    api_routers::update_zo_settings_root(path, |document| {
        let root = Value::Object(document.clone());
        let current = match role {
            Role::Coding => coding_reference(&root)?,
            Role::Judgment => reference(&root, connections::JUDGMENT_CONNECTION_KEY)?,
        };
        let recovery = role == Role::Judgment
            && expected.is_none()
            && judgment_provider::Provider::from_root(&root).is_err();
        if current.as_ref() != expected && !recovery {
            return Err("settings_changed".to_string());
        }
        if current.is_none() {
            return Ok(());
        }
        clear_in(document, role)
    })
}

pub(crate) fn uses_connection(root: &Value, id: &str) -> bool {
    root.get("smart")
        .and_then(|smart| smart.get(connections::JUDGMENT_CONNECTION_KEY))
        .and_then(|reference| reference.get("id"))
        .and_then(Value::as_str)
        == Some(id)
        || coding_reference(root)
            .ok()
            .flatten()
            .is_some_and(|reference| reference.id == id)
}

/// Whether `zo models --json` (`listed`) names `model`: a login model's id,
/// or a router model as zo spells it (`<provider>/<model>`).
fn zo_lists(listed: &Value, model: &str) -> bool {
    let rows = |key: &str| {
        listed
            .get(key)
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
    };
    rows("models")
        .iter()
        .any(|row| row.get("id").and_then(Value::as_str) == Some(model))
        || rows("customProviders").iter().any(|provider| {
            provider
                .get("models")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .any(|listed| listed.as_str() == Some(model))
        })
}

/// The saved connection a router model (`<name>/<model>`) belongs to, when
/// the settings keep one under that name with that model.
fn connection_of(root: &Value, model: &str) -> Result<Option<Reference>, String> {
    let Some((name, wanted)) = model.split_once('/') else {
        return Ok(None);
    };
    for row in root
        .get("providers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if !row
            .get("name")
            .and_then(Value::as_str)
            .is_some_and(|held| connections::names_match(held, name))
        {
            continue;
        }
        let declared = row
            .get("models")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|model| {
                model
                    .as_str()
                    .or_else(|| model.get("id").and_then(Value::as_str))
            })
            .find(|declared| declared.eq_ignore_ascii_case(wanted));
        if let Some(declared) = declared {
            let metadata = connections::metadata(row).map_err(|error| error.to_string())?;
            return Ok(Some(Reference {
                id: metadata.id,
                generation: metadata.generation,
                model: declared.to_string(),
                protocol: judgment_provider::Protocol::OpenaiJson,
            }));
        }
    }
    Ok(None)
}

/// A model `zo models --json` lists (`listed`), made the default of new zo
/// sessions: `model` is what a new session reads. A router model the
/// settings keep as a connection goes through that connection
/// ([`apply_coding`]); any other — a login model, or a router zo reads from
/// its own environment — is written as zo names it, and no coding connection
/// claims it. `expected` is the `model` the person saw; a file that moved
/// since is refused untouched.
pub fn apply_coding_model(
    path: &Path,
    model: &str,
    expected: Option<&str>,
    listed: &Value,
) -> Result<(), String> {
    if model.is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
        return Err("provider_model".to_string());
    }
    if !zo_lists(listed, model) {
        return Err("provider_model".to_string());
    }
    let current = |document: &Map<String, Value>| {
        document
            .get("model")
            .and_then(Value::as_str)
            .map(str::to_string)
    };
    let root = root(path)?;
    if let Some(reference) = connection_of(&root, model)? {
        let Value::Object(document) = &root else {
            return Err("provider_settings".to_string());
        };
        if current(document).as_deref() != expected {
            return Err("settings_changed".to_string());
        }
        return apply_coding(path, &reference);
    }
    api_routers::update_zo_settings_root(path, |document| {
        if current(document).as_deref() != expected {
            return Err("settings_changed".to_string());
        }
        let previous = document.get("model").cloned().unwrap_or(Value::Null);
        let smart = smart(document)?;
        smart.remove(connections::CODING_CONNECTION_KEY);
        smart.insert(connections::PREVIOUS_CODING_KEY.to_string(), previous);
        document.insert("model".to_string(), Value::String(model.to_string()));
        Ok(())
    })
}

/// The key the old judgment setting kept for `id`'s address, moved into the
/// row as the window's own router key — pressed by a person, never on its own.
pub fn adopt_legacy_judgment_key(
    path: &Path,
    id: &str,
    expected_generation: u64,
    keys: &dyn RouterKeys,
) -> Result<(), String> {
    let root = root(path)?;
    let row = connections::row(&root, id)
        .map_err(|error| error.to_string())?
        .clone();
    let metadata = connections::metadata(&row).map_err(|error| error.to_string())?;
    if metadata.generation != expected_generation {
        return Err("settings_changed".to_string());
    }
    let key = legacy_key_of(&root, &metadata, &row, keys)?.ok_or("no_key")?;
    let entry: ProviderEntry =
        serde_json::from_value(row.clone()).map_err(|error| error.to_string())?;
    api_routers::save_router(
        path,
        api_routers::RouterSave {
            preset_id: None,
            name: entry.name,
            base_url: entry.base_url,
            key: Some(key),
            models: entry.models,
            client_fingerprint: entry.client_fingerprint,
            headers: entry.headers,
            source_id: Some(id.to_string()),
            expected_generation: Some(expected_generation),
            auth: Some(metadata.auth),
        },
        keys,
    )
    .map_err(|refusal| refusal.message)?;
    Ok(())
}

/// The key the old judgment setting kept for `row`'s address, under either
/// way it could have asked — the item `provider_keychain_service` names for
/// that address with no connection behind it.
fn legacy_key_of(
    root: &Value,
    metadata: &connections::Metadata,
    row: &Value,
    keys: &dyn RouterKeys,
) -> Result<Option<String>, String> {
    let Some(model) = row
        .get("models")
        .and_then(Value::as_array)
        .and_then(|models| models.first())
        .and_then(|model| {
            model
                .as_str()
                .or_else(|| model.get("id").and_then(Value::as_str))
        })
    else {
        return Ok(None);
    };
    for protocol in [
        judgment_provider::Protocol::OpenaiJson,
        judgment_provider::Protocol::SystemOne,
    ] {
        let reference = Reference {
            id: metadata.id.clone(),
            generation: metadata.generation,
            model: model.to_string(),
            protocol,
        };
        let Ok(mut legacy) = connections::candidate(root, &reference) else {
            continue;
        };
        legacy.connection = None;
        let held = keys
            .read(&crate::typesafe_settings::provider_keychain_service(
                &legacy,
            ))
            .map_err(|error| error.message)?
            .filter(|key| !key.trim().is_empty());
        if held.is_some() {
            return Ok(held);
        }
    }
    Ok(None)
}

#[cfg(test)]
#[path = "model_connections_tests.rs"]
mod tests;
