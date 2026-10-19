use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, Once, OnceLock};

use local_judgment::{Binding, Manager, Snapshot, SourceInput, SourceInspection};
use tauri::{AppHandle, Emitter, Manager as TauriManager};

use crate::{AppState, api_routers, typesafe_settings};

static SERVICE: OnceLock<Mutex<Option<Arc<Manager>>>> = OnceLock::new();
static RESTORE_AT_BOOT: Once = Once::new();
static SHUTTING_DOWN: AtomicBool = AtomicBool::new(false);

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

pub(crate) fn restore_at_boot(app: &AppHandle) {
    let app = app.clone();
    RESTORE_AT_BOOT.call_once(|| {
        tauri::async_runtime::spawn(async move {
            let Some(manager) = boot_service(
                &SHUTTING_DOWN,
                || service(&app),
                |manager| manager.shutdown(),
            ) else {
                return;
            };
            let Some(path) = api_routers::zo_settings_path() else {
                return;
            };
            let Ok(root) = api_routers::read_zo_settings_root(&path) else {
                return;
            };
            let document = serde_json::Value::Object(root);
            let allowed = manager
                .restore_provider_identity()
                .is_some_and(|identity| restore_allowed(&document, &identity));
            let guard_path = path.clone();
            let _ = manager.restore_guarded(
                allowed,
                (
                    move |provider, expected, binding| {
                        save_checked_provider(&path, provider, expected, binding)
                    },
                    move |action| guard_provider(&guard_path, action),
                ),
            );
        });
    });
}

pub(crate) fn shutdown() {
    SHUTTING_DOWN.store(true, Ordering::Release);
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
                move |action| guard_provider(&guard_path, action),
            ),
        )
        .map_err(|error| error.to_string())
}

fn guard_provider(
    path: &Path,
    action: &mut dyn FnMut() -> local_judgment::Result<()>,
) -> local_judgment::Result<()> {
    let mut action_error = None;
    typesafe_settings::with_provider_locked(path, || {
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

fn boot_service<Service>(
    closing: &AtomicBool,
    open: impl FnOnce() -> Result<Service, String>,
    close: impl FnOnce(&Service),
) -> Option<Service> {
    if closing.load(Ordering::Acquire) {
        return None;
    }
    let service = open().ok()?;
    if closing.load(Ordering::Acquire) {
        close(&service);
        return None;
    }
    Some(service)
}

fn restore_allowed(document: &serde_json::Value, intent_identity: &str) -> bool {
    let Ok(provider) = judgment_provider::Provider::from_root(document) else {
        return false;
    };
    provider.kind == judgment_provider::ProviderKind::Local
        && provider.identity() == intent_identity
        && typesafe_settings::JevSwitch::of(document).on
        && document
            .get(zerocode_core::jev::SMART_SETTINGS_KEY)
            .and_then(|smart| smart.get(local_judgment::MANAGED_MODEL_KEY))
            .and_then(|binding| serde_json::from_value::<Binding>(binding.clone()).ok())
            .is_some_and(|binding| binding.matches(&provider))
}

#[cfg(test)]
mod tests {
    use serde_json::{Map, Value, json};

    use super::restore_allowed;

    #[test]
    fn boot_service_cannot_outlive_shutdown() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

        let closing = AtomicBool::new(true);
        let opened = AtomicUsize::new(0);
        let stopped = AtomicUsize::new(0);
        let service = super::boot_service(
            &closing,
            || {
                opened.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |_| {
                stopped.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert!(
            service.is_none(),
            "a boot service must not open after shutdown"
        );
        assert_eq!(opened.load(Ordering::SeqCst), 0);
        assert_eq!(stopped.load(Ordering::SeqCst), 0);
        closing.store(false, Ordering::SeqCst);
        let service = super::boot_service(
            &closing,
            || {
                opened.fetch_add(1, Ordering::SeqCst);
                closing.store(true, Ordering::SeqCst);
                Ok(())
            },
            |_| {
                stopped.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert!(service.is_none());
        assert_eq!(opened.load(Ordering::SeqCst), 1);
        assert_eq!(stopped.load(Ordering::SeqCst), 1);
        closing.store(false, Ordering::SeqCst);
        let service = super::boot_service(
            &closing,
            || {
                opened.fetch_add(1, Ordering::SeqCst);
                Ok(())
            },
            |_| {
                stopped.fetch_add(1, Ordering::SeqCst);
            },
        );
        assert!(service.is_some());
        assert_eq!(opened.load(Ordering::SeqCst), 2);
        assert_eq!(stopped.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn restore_gate_uses_the_window_switch_and_matching_local_binding() {
        let catalog = local_judgment::Catalog::parse(include_str!(
            "../../../local-judgment/manifests/models.json"
        ))
        .unwrap();
        let model = &catalog.models[0];
        let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).unwrap();
        let provider = judgment_provider::Provider {
            kind: judgment_provider::ProviderKind::Local,
            protocol: judgment_provider::Protocol::SystemOne,
            base_url: format!("http://{}", listener.local_addr().unwrap()),
            model: model.id.clone(),
            requires_auth: false,
        };
        let binding = local_judgment::Binding {
            model_id: model.id.clone(),
            revision: model.revision.clone(),
            provider_identity: provider.identity(),
        };
        let mut smart = Map::new();
        zerocode_core::jev::door::switch_on(&mut smart).unwrap();
        smart.insert("judgmentProvider".to_owned(), json!(provider));
        smart.insert(local_judgment::MANAGED_MODEL_KEY.to_owned(), json!(binding));
        let enabled = json!({"smart": smart});
        let mut off = enabled.clone();
        zerocode_core::jev::door::switch_off(off["smart"].as_object_mut().unwrap()).unwrap();
        assert!(!restore_allowed(&off, &provider.identity()));
        let mut unmanaged = enabled.clone();
        unmanaged["smart"]["judgmentProvider"] =
            json!(judgment_provider::Provider::from_root(&json!({})).unwrap());
        assert!(!restore_allowed(&unmanaged, &provider.identity()));
        let mut mismatch = enabled.clone();
        mismatch["smart"][local_judgment::MANAGED_MODEL_KEY]["modelId"] =
            json!(format!("{}-other", model.id));
        assert!(!restore_allowed(&mismatch, &provider.identity()));
        let mut missing = enabled.clone();
        missing["smart"][local_judgment::MANAGED_MODEL_KEY] = Value::Null;
        assert!(!restore_allowed(&missing, &provider.identity()));
        assert!(!restore_allowed(
            &enabled,
            &judgment_provider::Provider::from_root(&json!({}))
                .unwrap()
                .identity()
        ));
        assert!(
            restore_allowed(&enabled, &provider.identity()),
            "local enabled matching binding must permit restoration"
        );
        let mut legacy = enabled;
        let feature = &zerocode_core::jev::JEV_USES[0];
        legacy["smart"][feature.setting] = json!(feature.recommended.key());
        legacy["smart"]["jev"]
            .as_object_mut()
            .unwrap()
            .remove(zerocode_core::jev::door::ENABLED_SETTING);
        assert!(
            restore_allowed(&legacy, &provider.identity()),
            "legacy settings must use the window switch semantics"
        );
    }
}
