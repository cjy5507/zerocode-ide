//! TypeSafe (Jev) settings IPC — the key and the decision shadow's switch.
use crate::api_routers::{self, Keychain, RouterRefusal};
use crate::typesafe_settings::{self, TypeSafeCheck, TypeSafeSettings};

fn settings_path() -> Result<std::path::PathBuf, RouterRefusal> {
    api_routers::zo_settings_path()
        .ok_or_else(|| RouterRefusal::from("설정 경로를 찾을 수 없습니다".to_string()))
}

fn settings_now() -> Result<TypeSafeSettings, RouterRefusal> {
    let keychain = Keychain::of_this_machine();
    typesafe_settings::read_settings(&settings_path()?, &keychain, keychain.keeps_keys())
}

/// Whether a key could be kept here, whether one is saved, and the switch.
#[tauri::command(async)]
pub(crate) fn typesafe_settings() -> Result<TypeSafeSettings, RouterRefusal> {
    settings_now()
}

/// Keep the key in the keychain item every zo reads.
#[tauri::command(async)]
pub(crate) fn save_typesafe_key(key: String) -> Result<TypeSafeSettings, RouterRefusal> {
    typesafe_settings::save_key(&key, &Keychain::of_this_machine())?;
    settings_now()
}

/// Forget the saved key.
#[tauri::command(async)]
pub(crate) fn remove_typesafe_key() -> Result<TypeSafeSettings, RouterRefusal> {
    typesafe_settings::remove_key(&Keychain::of_this_machine())?;
    settings_now()
}

/// Turn the decision shadow on (`shadow`) or off (`off`) in zo's settings.
#[tauri::command(async)]
pub(crate) fn set_decision_shadow(mode: String) -> Result<TypeSafeSettings, RouterRefusal> {
    typesafe_settings::set_decision_shadow(&settings_path()?, &mode)?;
    settings_now()
}

/// Turn the window's browser recovery off, to record only, or on.
#[tauri::command(async)]
pub(crate) fn set_browser_action(mode: String) -> Result<TypeSafeSettings, RouterRefusal> {
    typesafe_settings::set_browser_action(&settings_path()?, &mode)?;
    settings_now()
}

/// Ask the installed zo whether System One answers with the key it would use:
/// `zo decision-shadow check --json`, the shadow's own question about a task
/// nobody wrote. A check nothing answered is still an answer (its failure
/// token); only a zo that printed no answer at all is an error.
#[tauri::command]
pub(crate) async fn check_typesafe_key() -> Result<TypeSafeCheck, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let home = dirs::home_dir().ok_or_else(|| "no home directory".to_string())?;
        let bin = crate::zo_companion::zo_path_under(&home);
        if !bin.exists() {
            return Err(format!("zo not installed at {}", bin.display()));
        }
        let output = crate::proc::quiet_command(&bin)
            .args(typesafe_settings::ZO_KEY_CHECK_ARGS)
            .output()
            .map_err(|error| error.to_string())?;
        typesafe_settings::read_check(&output.stdout).ok_or_else(|| {
            format!(
                "zo {} exited {}: {}",
                typesafe_settings::ZO_KEY_CHECK_ARGS.join(" "),
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            )
        })
    })
    .await
    .map_err(|error| error.to_string())?
}
