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

#[tauri::command(async)]
pub(crate) fn api_router_providers() -> Result<Vec<ProviderEntry>, String> {
    let path = api_routers::zo_settings_path().ok_or("설정 경로를 찾을 수 없습니다")?;
    api_routers::read_providers_file(&path)
}

#[tauri::command(async)]
#[allow(clippy::too_many_arguments)] // Tauri command arguments are the public IPC wire.
pub(crate) async fn test_router_connection(
    base_url: String,
    auth: RouterAuth,
    key: Option<String>,
    models_path: Option<String>,
    id_field: Option<String>,
    context_field: Option<String>,
    client_fingerprint: Option<String>,
    headers: Option<std::collections::BTreeMap<String, String>>,
) -> Result<Vec<DiscoveredModel>, String> {
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

#[tauri::command(async)]
#[allow(clippy::too_many_arguments)] // Tauri command arguments are the public IPC wire.
pub(crate) fn save_api_router(
    preset_id: Option<String>,
    name: String,
    base_url: String,
    key: Option<String>,
    models: Vec<ProviderModel>,
    client_fingerprint: Option<String>,
    headers: Option<std::collections::BTreeMap<String, String>>,
) -> Result<Vec<ProviderEntry>, RouterRefusal> {
    let path = api_routers::zo_settings_path()
        .ok_or_else(|| RouterRefusal::from("설정 경로를 찾을 수 없습니다".to_string()))?;
    api_routers::save_router(
        &path,
        api_routers::RouterSave {
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
}

/// A row is removed by its name; the key it read is found on the row itself.
#[tauri::command(async)]
pub(crate) fn remove_api_router(name: String) -> Result<Vec<ProviderEntry>, String> {
    let path = api_routers::zo_settings_path().ok_or("설정 경로를 찾을 수 없습니다")?;
    api_routers::remove_router(&path, &name, &api_routers::Keychain::of_this_machine())
}
