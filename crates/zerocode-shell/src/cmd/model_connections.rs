use judgment_provider::connections::Reference;

use crate::api_routers::{self, Keychain};
use crate::model_connections::{self, Catalog, Check, Role};

fn path() -> Result<std::path::PathBuf, String> {
    api_routers::zo_settings_path().ok_or_else(|| "provider_settings".to_string())
}

#[tauri::command]
pub(crate) async fn model_connections() -> Result<Catalog, String> {
    tauri::async_runtime::spawn_blocking(|| {
        model_connections::catalog(&path()?, &Keychain::of_this_machine())
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn check_model_connection(
    reference: Reference,
    expected_provider_identity: String,
) -> Result<Check, String> {
    tauri::async_runtime::spawn_blocking(move || {
        model_connections::check(
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
            Role::Coding => model_connections::apply_coding(&path, &reference)?,
            Role::Judgment => model_connections::apply_judgment(
                &path,
                &reference,
                &expected_provider_identity,
                &keys,
                &crate::systemone::Wire::of_this_machine(),
            )?,
        }
        super::managed_judgment::settings_changed();
        model_connections::catalog(&path, &keys)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tauri::command]
pub(crate) async fn clear_model_connection(role: Role) -> Result<Catalog, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let path = path()?;
        model_connections::clear(&path, role)?;
        super::managed_judgment::settings_changed();
        model_connections::catalog(&path, &Keychain::of_this_machine())
    })
    .await
    .map_err(|error| error.to_string())?
}
