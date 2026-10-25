//! Non-secret route bindings for credentials supplied at launch.
//! A reload may keep a launch credential, but cannot widen the routes allowed to use it.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read as _;
use std::sync::{OnceLock, RwLock};

use judgment_provider::connections::{self, LaunchRoutes};
use serde_json::Value;

fn bindings() -> &'static RwLock<BTreeMap<String, BTreeSet<String>>> {
    static BINDINGS: OnceLock<RwLock<BTreeMap<String, BTreeSet<String>>>> = OnceLock::new();
    BINDINGS.get_or_init(Default::default)
}

/// The caller holds the common settings lock while pairing this document with a key.
pub(crate) fn read_settings(path: &std::path::Path) -> Result<Value, crate::ApiError> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?.take(judgment_provider::MAX_RESPONSE_BYTES as u64 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > judgment_provider::MAX_RESPONSE_BYTES { return Err(crate::ApiError::ConnectionChanged); }
    let root: Value = serde_json::from_slice(&bytes)?;
    if !root.is_object() { return Err(crate::ApiError::ConnectionChanged); }
    Ok(root)
}

pub(crate) fn judgment_credential(path: &std::path::Path, provider: &judgment_provider::Provider) -> Result<Option<(String, crate::providers::KeySource)>, crate::ApiError> {
    let _guard = zerocode_core::settings_file_lock::SettingsFileLock::acquire(path)?;
    let root = read_settings(path)?;
    let current = judgment_provider::Provider::from_root(&root).map_err(|_| crate::ApiError::ConnectionChanged)?;
    if current.identity() != provider.identity() { return Err(crate::ApiError::ConnectionChanged); }
    let connection = provider.connection.as_ref().ok_or(crate::ApiError::ConnectionChanged)?;
    let row = connections::row(&root, &connection.reference.id).map_err(|_| crate::ApiError::ConnectionChanged)?;
    let stamp = connections::route_stamp(row).map_err(|_| crate::ApiError::ConnectionChanged)?;
    crate::providers::connection_credential(&connection.auth_env, &stamp)
}

fn add_rows(routes: &mut BTreeMap<String, BTreeSet<String>>, rows: Option<&Vec<Value>>) {
    for row in rows.into_iter().flatten() {
        if let (Some(name), Ok(stamp)) = (row.get("auth_env").and_then(Value::as_str), connections::route_stamp(row)) {
            routes.entry(name.to_string()).or_default().insert(stamp);
        }
    }
}

fn settings_routes() -> BTreeMap<String, BTreeSet<String>> {
    let mut routes = BTreeMap::new();
    let path = core_types::paths::default_config_home().join("settings.json");
    // A writer holding the lock past the poll must not empty the routes a launch key is held to.
    // Writers replace or truncate the whole file, so the unlocked read sees a committed document or fails to parse.
    let _guard = zerocode_core::settings_file_lock::SettingsFileLock::acquire(&path).ok();
    if let Ok(file) = std::fs::File::open(&path) {
        let mut bytes = Vec::new();
        if file.take(judgment_provider::MAX_RESPONSE_BYTES as u64 + 1).read_to_end(&mut bytes).is_ok()
            && bytes.len() <= judgment_provider::MAX_RESPONSE_BYTES {
            if let Ok(root) = serde_json::from_slice::<Value>(&bytes) { add_rows(&mut routes, root.get("providers").and_then(Value::as_array)); }
        }
    }
    // An operator-exported catalog is also part of the launch's declared routes.
    if let Ok(raw) = std::env::var(crate::providers::CUSTOM_PROVIDERS_ENV) {
        if raw.len() <= judgment_provider::MAX_RESPONSE_BYTES {
            if let Ok(rows) = serde_json::from_str::<Value>(&raw) { add_rows(&mut routes, rows.as_array()); }
        }
    }
    routes
}

/// Called alongside adoption, before the process starts threads. The optional
/// window envelope closes the interval between the window's read and CLI startup.
pub(crate) fn capture(names: &[String], prefix: &str) {
    if names.is_empty() { return; }
    let handed = if prefix.starts_with(connections::ROUTER_KEY_ENV_PREFIX) || connections::ROUTER_KEY_ENV_PREFIX.starts_with(prefix) {
        let value = std::env::var(connections::LAUNCH_ROUTES_ENV);
        std::env::remove_var(connections::LAUNCH_ROUTES_ENV);
        match value {
            Ok(value) => Some(serde_json::from_str::<LaunchRoutes>(&value).ok().filter(|routes| routes.version == connections::LAUNCH_ROUTES_VERSION)),
            Err(std::env::VarError::NotPresent) => None,
            Err(_) => Some(None),
        }
    } else { None };
    let declared = settings_routes();
    let mut held = bindings().write().unwrap_or_else(std::sync::PoisonError::into_inner);
    for name in names {
        let allowed = match &handed {
            Some(Some(handed)) => handed.routes.get(name).map_or_else(|| declared.get(name).cloned().unwrap_or_default(), |routes| routes.iter().cloned().collect()),
            Some(None) => BTreeSet::new(),
            None => declared.get(name).cloned().unwrap_or_default(),
        };
        held.insert(name.clone(), allowed);
    }
}

pub(crate) fn permits(name: &str, stamp: &str) -> bool {
    bindings().read().unwrap_or_else(std::sync::PoisonError::into_inner).get(name).is_some_and(|routes| routes.contains(stamp))
}

#[cfg(test)]
pub(crate) fn clear() {
    bindings().write().unwrap_or_else(std::sync::PoisonError::into_inner).clear();
}
