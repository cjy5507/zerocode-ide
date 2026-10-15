use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use local_judgment::{Binding, Manager, Snapshot, SourceInput, SourceInspection};
use tauri::{AppHandle, Emitter, Manager as TauriManager};

use crate::{AppState, api_routers, typesafe_settings};

static SERVICE: OnceLock<Mutex<Option<Arc<Manager>>>> = OnceLock::new();

fn service(app: &AppHandle) -> Result<Arc<Manager>, String> {
    let mut service = SERVICE
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(manager) = &*service {
        return Ok(manager.clone());
    }
    let settings = api_routers::zo_settings_path().ok_or("settings_unavailable")?;
    let handle = app.clone();
    let manager = Manager::new(
        app.state::<AppState>().local_data_root(),
        &settings,
        move |snapshot| {
            let _ = handle.emit_to(
                crate::MAIN_WINDOW_LABEL,
                "managed-judgment:changed",
                snapshot,
            );
        },
    )
    .map_err(|error| error.to_string())?;
    *service = Some(manager.clone());
    Ok(manager)
}

pub(crate) fn shutdown() {
    if let Some(service) = SERVICE.get() {
        let service = service
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(manager) = service.as_ref() {
            manager.shutdown();
        }
    }
}

pub(crate) fn settings_changed() {
    let manager = SERVICE.get().and_then(|service| {
        service
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    });
    if let Some(manager) = manager {
        manager.notify_settings_changed();
    }
}

#[tauri::command(async)]
pub(crate) fn managed_judgment_status(app: AppHandle) -> Result<Snapshot, String> {
    Ok(service(&app)?.snapshot())
}

#[tauri::command]
pub(crate) async fn managed_judgment_inspect_source(
    app: AppHandle,
    source: SourceInput,
    request_id: Option<String>,
) -> Result<SourceInspection, String> {
    service(&app)?
        .inspect_source_with_request(source, request_id)
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub(crate) fn managed_judgment_install_source(
    app: AppHandle,
    inspection_id: String,
    instance_id: String,
    expected_catalog_fingerprint: String,
    destination_directory: Option<String>,
) -> Result<Snapshot, String> {
    service(&app)?
        .install_source(
            &inspection_id,
            &instance_id,
            &expected_catalog_fingerprint,
            destination_directory.map(PathBuf::from),
        )
        .map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub(crate) fn managed_judgment_register_catalog(
    app: AppHandle,
    instance_id: String,
    expected_catalog_fingerprint: String,
    catalog_json: String,
) -> Result<Snapshot, String> {
    service(&app)?
        .register_catalog(&instance_id, &expected_catalog_fingerprint, &catalog_json)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn managed_judgment_prepare_runtime(app: AppHandle) -> Result<Snapshot, String> {
    service(&app)?
        .prepare_runtime()
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn managed_judgment_prepare_model_runtime(
    app: AppHandle,
    model_id: String,
    revision: String,
) -> Result<Snapshot, String> {
    service(&app)?
        .prepare_model_runtime(&model_id, &revision)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn managed_judgment_install(
    app: AppHandle,
    model_id: String,
    revision: String,
    source_directory: Option<String>,
) -> Result<Snapshot, String> {
    service(&app)?
        .install(&model_id, &revision, source_directory.map(PathBuf::from))
        .map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub(crate) fn managed_judgment_cancel(
    app: AppHandle,
    operation_id: String,
) -> Result<Snapshot, String> {
    service(&app)?
        .cancel(&operation_id)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn managed_judgment_start(
    app: AppHandle,
    model_id: String,
    revision: String,
) -> Result<Snapshot, String> {
    service(&app)?
        .start(&model_id, &revision)
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub(crate) async fn managed_judgment_check(
    app: AppHandle,
    server_id: String,
) -> Result<Snapshot, String> {
    service(&app)?
        .check(&server_id)
        .map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub(crate) fn managed_judgment_switch_model(
    app: AppHandle,
    model_id: String,
    revision: String,
    server_id: String,
    expected_provider_identity: String,
    instance_id: String,
) -> Result<Snapshot, String> {
    let path = api_routers::zo_settings_path().ok_or("settings_unavailable")?;
    let guard_path = path.clone();
    service(&app)?
        .switch_model_guarded(
            &model_id,
            &revision,
            &server_id,
            &expected_provider_identity,
            &instance_id,
            (
                move |provider, expected, binding| {
                    save_checked_provider(&path, provider, expected, binding)
                },
                move |action| {
                    let mut action_error = None;
                    typesafe_settings::with_provider_locked(&guard_path, || {
                        action().map_err(|error| {
                            action_error = Some(error);
                            error.to_string()
                        })
                    })
                    .map_err(|error| {
                        action_error.unwrap_or_else(|| {
                            if error == "settings_changed" {
                                local_judgment::Error::SettingsChanged
                            } else {
                                local_judgment::Error::Storage
                            }
                        })
                    })
                },
            ),
        )
        .map_err(|error| error.to_string())
}

fn save_checked_provider(
    path: &Path,
    provider: &judgment_provider::Provider,
    expected: &str,
    binding: &Binding,
) -> local_judgment::Result<()> {
    typesafe_settings::set_provider_if_identity(path, provider.clone(), expected, Some(binding))
        .map_err(|error| {
            if error == "settings_changed" {
                local_judgment::Error::SettingsChanged
            } else {
                local_judgment::Error::Storage
            }
        })
}

#[tauri::command(async)]
pub(crate) fn managed_judgment_apply(
    app: AppHandle,
    server_id: String,
    expected_provider_identity: String,
) -> Result<Snapshot, String> {
    let path = api_routers::zo_settings_path().ok_or("settings_unavailable")?;
    service(&app)?
        .apply(
            &server_id,
            &expected_provider_identity,
            |provider, expected, binding| save_checked_provider(&path, provider, expected, binding),
        )
        .map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub(crate) fn managed_judgment_stop(app: AppHandle, server_id: String) -> Result<Snapshot, String> {
    service(&app)?
        .stop(&server_id)
        .map_err(|error| error.to_string())
}

#[tauri::command(async)]
pub(crate) fn managed_judgment_remove(
    app: AppHandle,
    model_id: String,
    revision: String,
) -> Result<Snapshot, String> {
    let path = api_routers::zo_settings_path().ok_or("settings_unavailable")?;
    let service = service(&app)?;
    service
        .remove_guarded(&model_id, &revision, |remove| {
            typesafe_settings::with_provider_locked(&path, || {
                remove().map_err(|error| error.to_string())
            })
            .map_err(|error| match error.as_str() {
                "model_configured" => local_judgment::Error::Configured,
                "settings_changed" => local_judgment::Error::SettingsChanged,
                _ => local_judgment::Error::Storage,
            })
        })
        .map_err(|error| error.to_string())
}
