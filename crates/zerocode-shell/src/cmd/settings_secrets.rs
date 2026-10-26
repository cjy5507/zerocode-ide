//! Secrets written as plain text in zo's settings file (t-76698): the names
//! the settings line shows, and the move into the keychain a person presses.
//! The rule and the value's road are `crate::settings_secrets`.

use crate::api_routers::{self, Keychain};
use crate::settings_secrets::{self, MoveOutcome, SecretNotice, SettingsFile, ZoDoor};
use tauri::State;

use crate::*;

/// The person's settings file, with the folder it is trusted from.
fn settings_path() -> Result<SettingsFile, String> {
    api_routers::zo_settings_root_and_path()
        .and_then(|(root, path)| SettingsFile::under(root, path))
        .ok_or_else(|| "설정 경로를 찾을 수 없습니다".to_string())
}

/// The names of the secrets the settings file holds as plain text, or nothing
/// — none there, or a zo that does not know the door. Asked once the settings
/// are drawn; it never fails the page.
#[tauri::command]
pub(crate) async fn settings_secret_literals(
    state: State<'_, AppState>,
) -> Result<Option<SecretNotice>, String> {
    let config = state.config_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let Ok(settings) = settings_path() else {
            return None;
        };
        settings_secrets::notice(&ZoDoor::of_this_window(&config)?, &settings)
    })
    .await
    .map_err(|error| error.to_string())
}

/// 「키체인으로 옮기기」, pressed by a person: each plain-text secret into the
/// keychain item zo reads, then the cell rewritten as its reference by zo.
#[tauri::command]
pub(crate) async fn move_settings_secrets(
    state: State<'_, AppState>,
) -> Result<MoveOutcome, String> {
    let config = state.config_root().to_path_buf();
    tauri::async_runtime::spawn_blocking(move || {
        let settings = settings_path()?;
        let zo = ZoDoor::of_this_window(&config)
            .ok_or_else(|| "홈 폴더를 찾을 수 없습니다".to_string())?;
        Ok(settings_secrets::move_all(
            &zo,
            &settings,
            &Keychain::of_this_machine(),
        ))
    })
    .await
    .map_err(|error| error.to_string())?
}
