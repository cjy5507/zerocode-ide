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
    pub uses: Vec<Role>,
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
        connections.push(ConnectionRow {
            id: metadata.id,
            generation: metadata.generation,
            provider,
            auth: metadata.auth,
            probe_headers: api_routers::probe_headers(row)?,
            key_available,
            uses,
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
        let smart = smart(document)?;
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
