use judgment_provider::connections::Reference;

use crate::api_routers::{self, Keychain};
use crate::model_connection_checks as checks;
use crate::model_connection_store::{self as store, Catalog, Check, Role};

fn path() -> Result<std::path::PathBuf, String> {
    api_routers::zo_settings_path().ok_or_else(|| "provider_settings".to_string())
}

#[tauri::command]
pub(crate) async fn model_connections() -> Result<Catalog, String> {
    tauri::async_runtime::spawn_blocking(|| store::catalog(&path()?, &Keychain::of_this_machine()))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn check_model_connection(
    reference: Reference,
    expected_provider_identity: String,
) -> Result<Check, String> {
    tauri::async_runtime::spawn_blocking(move || {
        store::check(
            &path()?,
            &reference,
            &expected_provider_identity,
            &Keychain::of_this_machine(),
            &crate::systemone::Wire::of_this_machine(),
        )
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn select_model_connection(
    role: Role,
    reference: Reference,
    expected_provider_identity: String,
) -> Result<Catalog, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = path()?;
        let keys = Keychain::of_this_machine();
        match role {
            Role::Coding => store::apply_coding(&path, &reference)?,
            Role::Judgment => store::apply_judgment(
                &path,
                &reference,
                &expected_provider_identity,
                &keys,
                &crate::systemone::Wire::of_this_machine(),
            )?,
        }
        super::managed_judgment::settings_changed();
        store::catalog(&path, &keys)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// zo's own model list — the catalog the window already reads and caches for
/// the model menu (`slash_catalog::zo_model_list`), or a forced live refresh
/// when `fresh` is `Some(true)` (such as the setup wizard's login check).
#[tauri::command]
pub(crate) async fn zo_model_list(fresh: Option<bool>) -> Result<serde_json::Value, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let home = dirs::home_dir().ok_or("home")?;
        if fresh == Some(true) {
            crate::slash_catalog::refresh_zo_models();
        }
        let list = crate::slash_catalog::zo_model_list(&home);
        if fresh == Some(true) {
            crate::summon_lineup::refresh();
        }
        Ok(list)
    })
    .await
    .map_err(|error| error.to_string())?
}

/// A model zo lists, made the coding default of new zo sessions.
#[tauri::command]
pub(crate) async fn select_coding_model(
    model: String,
    expected_model: Option<String>,
) -> Result<Catalog, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = path()?;
        let home = dirs::home_dir().ok_or("home")?;
        let listed = crate::slash_catalog::zo_model_list(&home);
        store::apply_coding_model(&path, &model, expected_model.as_deref(), &listed)?;
        store::catalog(&path, &Keychain::of_this_machine())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// The old judgment setting's key for a connection's address, moved into the
/// connection by a person's press (t-75762 ⑥).
#[tauri::command]
pub(crate) async fn adopt_legacy_judgment_key(
    id: String,
    expected_generation: u64,
) -> Result<Catalog, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = path()?;
        let keys = Keychain::of_this_machine();
        store::adopt_legacy_judgment_key(&path, &id, expected_generation, &keys)?;
        // The key moved under the row: whether zo can use it changed.
        crate::summon_lineup::refresh();
        super::managed_judgment::settings_changed();
        store::catalog(&path, &keys)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn clear_model_connection(
    role: Role,
    expected_reference: Option<Reference>,
) -> Result<Catalog, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = path()?;
        store::clear(&path, role, expected_reference.as_ref())?;
        super::managed_judgment::settings_changed();
        store::catalog(&path, &Keychain::of_this_machine())
    })
    .await
    .map_err(|error| error.to_string())?
}

/// The window's own record of the last check each connection had
/// (t-76500): `<local data>/model-connection-checks.json`.
fn checks_path(state: &crate::AppState) -> std::path::PathBuf {
    state
        .local_data_root()
        .join(crate::app_paths::artifact_file::MODEL_CONNECTION_CHECKS)
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| {
            u64::try_from(since.as_millis()).unwrap_or(u64::MAX)
        })
}

/// The ids of the connections saved now: a record of any other goes.
fn saved_ids() -> Result<std::collections::BTreeSet<String>, String> {
    let root = api_routers::read_zo_settings_root(&path()?)?;
    Ok(root
        .get("providers")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|row| judgment_provider::connections::metadata(row).ok())
        .map(|metadata| metadata.id)
        .collect())
}

#[tauri::command]
pub(crate) async fn remembered_model_connection_checks(
    state: tauri::State<'_, crate::AppState>,
) -> Result<checks::Remembered, String> {
    let path = checks_path(&state);
    tauri::async_runtime::spawn_blocking(move || Ok(checks::read(&path, now_ms())))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn remember_model_connection_check(
    state: tauri::State<'_, crate::AppState>,
    answer: checks::CheckAnswer,
) -> Result<checks::Remembered, String> {
    let path = checks_path(&state);
    tauri::async_runtime::spawn_blocking(move || {
        checks::remember_check(&path, &answer, &saved_ids()?, now_ms())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn remember_model_judgment_check(
    state: tauri::State<'_, crate::AppState>,
    answer: checks::JudgmentAnswer,
) -> Result<checks::Remembered, String> {
    let path = checks_path(&state);
    tauri::async_runtime::spawn_blocking(move || {
        checks::remember_judgment(&path, &answer, &saved_ids()?, now_ms())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[derive(Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ReloadStatus {
    queued: usize,
    restart_required: usize,
    unknown: usize,
}

#[tauri::command]
pub(crate) async fn sync_model_connections(
    state: tauri::State<'_, crate::AppState>,
) -> Result<ReloadStatus, String> {
    let tasks: Vec<_> = crate::shell_runtime::model_connection_targets(&state)
        .into_iter()
        .map(|(_, channel)| {
            tauri::async_runtime::spawn_blocking(move || {
                crate::shell_runtime::reload_model_connection(channel)
            })
        })
        .collect();
    let mut status = ReloadStatus::default();
    for task in tasks {
        match task.await {
            Ok(Ok(true)) => status.queued += 1,
            Ok(Ok(false)) => status.restart_required += 1,
            Ok(Err(_)) | Err(_) => status.unknown += 1,
        }
    }
    Ok(status)
}

/// Keep that the judgment provider the settings name NOW was applied
/// (t-76598): read back off zo's settings after an apply, never taken from
/// the page, so the time is the one of what is actually written there.
#[tauri::command]
pub(crate) async fn remember_judgment_applied(
    state: tauri::State<'_, crate::AppState>,
) -> Result<checks::Remembered, String> {
    let path = checks_path(&state);
    tauri::async_runtime::spawn_blocking(move || {
        let provider = judgment_provider::Provider::read(&self::path()?)
            .map_err(|reason| reason.token().to_string())?;
        checks::remember_applied(&path, &provider.identity(), &saved_ids()?, now_ms())
    })
    .await
    .map_err(|error| error.to_string())?
}
