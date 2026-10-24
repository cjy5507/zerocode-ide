//! Saved model connections and explicit role assignments over the existing provider table.

use std::path::Path;

use judgment_provider::connections::{self, Reference};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use crate::api_routers::{self, ProviderEntry, RouterKeys};

const PREVIOUS_CODING: &str = "previousCodingSelection";
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

pub fn catalog(path: &Path, keys: &dyn RouterKeys) -> Result<Catalog, String> {
    let root = root(path)?;
    let coding = reference(&root, connections::CODING_CONNECTION_KEY)?;
    let judgment = reference(&root, connections::JUDGMENT_CONNECTION_KEY)?;
    let active =
        judgment_provider::Provider::from_root(&root).map_err(|error| error.to_string())?;
    let mut connections = Vec::new();
    for row in root
        .get("providers")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let provider: ProviderEntry =
            serde_json::from_value(row.clone()).map_err(|error| error.to_string())?;
        let metadata = connections::metadata(row).map_err(|error| error.to_string())?;
        let key_available = !provider.requires_auth
            || keys
                .read(&api_routers::router_keychain_service_for_env(
                    &provider.auth_env,
                ))
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
        let model = format!("{name}/{}", reference.model);
        persist_identity(document, &reference.id)?;
        let previous = document.get("model").cloned().unwrap_or(Value::Null);
        let smart = smart(document)?;
        if !smart.contains_key(connections::CODING_CONNECTION_KEY) {
            smart.insert(PREVIOUS_CODING.to_string(), previous);
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
            if smart.remove(connections::CODING_CONNECTION_KEY).is_some() {
                let previous = smart.remove(PREVIOUS_CODING).unwrap_or(Value::Null);
                if previous.is_null() {
                    document.remove("model");
                } else {
                    document.insert("model".to_string(), previous);
                }
            }
        }
    }
    Ok(())
}

pub fn clear(path: &Path, role: Role) -> Result<(), String> {
    api_routers::update_zo_settings_root(path, |root| clear_in(root, role))
}

pub(crate) fn uses_connection(root: &Value, id: &str) -> bool {
    [
        connections::CODING_CONNECTION_KEY,
        connections::JUDGMENT_CONNECTION_KEY,
    ]
    .iter()
    .any(|key| {
        root.get("smart")
            .and_then(|smart| smart.get(*key))
            .and_then(|reference| reference.get("id"))
            .and_then(Value::as_str)
            == Some(id)
    })
}
