use crate::api_routers::{
    self, DiscoveredModel, ProviderEntry, ProviderModel, RouterAuth, RouterPreset, RouterRefusal,
};

#[tauri::command(async)]
pub(crate) fn api_router_presets() -> Result<Vec<RouterPreset>, String> {
    api_routers::load_presets()
}

/// Whether this machine keeps router keys (the macOS Keychain). The pane
/// words its hint and offers its key field by this, so a window with no
/// keychain never promises one; the save still refuses a key there.
#[tauri::command]
pub(crate) fn api_router_keys_kept() -> bool {
    let _crumb = crate::crumbs::Command::enter("api_router_keys_kept");
    api_routers::Keychain::of_this_machine().keeps_keys()
}

#[tauri::command]
pub(crate) async fn api_router_providers() -> Result<Vec<ProviderEntry>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let path = api_routers::zo_settings_path().ok_or("설정 경로를 찾을 수 없습니다")?;
        api_routers::read_providers_file(&path)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command(async)]
#[allow(clippy::too_many_arguments)] // Tauri command arguments are the public IPC wire.
pub(crate) async fn test_router_connection(
    source_id: Option<String>,
    expected_generation: Option<u64>,
    base_url: String,
    auth: RouterAuth,
    key: Option<String>,
    models_path: Option<String>,
    id_field: Option<String>,
    context_field: Option<String>,
    client_fingerprint: Option<String>,
    headers: Option<std::collections::BTreeMap<String, String>>,
) -> Result<Vec<DiscoveredModel>, api_routers::ProbeFailure> {
    let key = if key.as_deref().is_none_or(|key| key.trim().is_empty()) {
        if let (Some(id), Some(generation)) = (source_id, expected_generation) {
            let base = base_url.clone();
            let auth = auth.clone();
            let fingerprint = client_fingerprint.clone();
            let headers = headers.clone();
            tauri::async_runtime::spawn_blocking(move || {
                let path = api_routers::zo_settings_path().ok_or("provider_settings")?;
                api_routers::saved_probe_key(
                    &path,
                    &id,
                    generation,
                    &api_routers::RouterProbe {
                        base_url: &base,
                        auth: &auth,
                        key: None,
                        models_path: None,
                        id_field: None,
                        context_field: None,
                        client_fingerprint: fingerprint.as_deref(),
                        headers: headers.as_ref(),
                    },
                    &api_routers::Keychain::of_this_machine(),
                )
            })
            .await
            .map_err(|error| error.to_string())??
        } else {
            key
        }
    } else {
        key
    };

    api_routers::test_router_endpoint(&api_routers::RouterProbe {
        base_url: &base_url,
        auth: &auth,
        key: key.as_deref(),
        models_path: models_path.as_deref(),
        id_field: id_field.as_deref(),
        context_field: context_field.as_deref(),
        client_fingerprint: client_fingerprint.as_deref(),
        headers: headers.as_ref(),
    })
    .await
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // Tauri command arguments are the public IPC wire.
pub(crate) async fn save_api_router(
    source_id: Option<String>,
    expected_generation: Option<u64>,
    auth: Option<judgment_provider::connections::Authentication>,
    preset_id: Option<String>,
    name: String,
    base_url: String,
    key: Option<String>,
    models: Vec<ProviderModel>,
    client_fingerprint: Option<String>,
    headers: Option<std::collections::BTreeMap<String, String>>,
) -> Result<Vec<ProviderEntry>, RouterRefusal> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = api_routers::zo_settings_path()
            .ok_or_else(|| RouterRefusal::from("설정 경로를 찾을 수 없습니다".to_string()))?;
        api_routers::save_router(
            &path,
            api_routers::RouterSave {
                source_id,
                expected_generation,
                auth,
                preset_id,
                name,
                base_url,
                key,
                models,
                client_fingerprint,
                headers,
            },
            &api_routers::Keychain::of_this_machine(),
        )
        // A connection saved: zo lists other models, and the worker rows
        // offer them as soon as zo has said so.
        .inspect(|_| crate::summon_lineup::refresh())
    })
    .await
    .map_err(|error| RouterRefusal::from(error.to_string()))?
}

/// A row is removed by its name; the key it read is found on the row itself.
#[tauri::command]
pub(crate) async fn remove_api_router(
    name: String,
    source_id: Option<String>,
    expected_generation: Option<u64>,
) -> Result<Vec<ProviderEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = api_routers::zo_settings_path().ok_or("설정 경로를 찾을 수 없습니다")?;
        let removed = match (source_id.as_deref(), expected_generation) {
            (Some(id), Some(generation)) => api_routers::remove_router_if_current(
                &path,
                &name,
                Some((id, generation)),
                &api_routers::Keychain::of_this_machine(),
            ),
            (None, None) => {
                api_routers::remove_router(&path, &name, &api_routers::Keychain::of_this_machine())
            }
            _ => Err("settings_changed".to_string()),
        };
        removed.inspect(|_| crate::summon_lineup::refresh())
    })
    .await
    .map_err(|error| error.to_string())?
}
