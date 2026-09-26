//! The Computer Use pane's generator card (t-9537, t-10372): which road
//! typing into a page's fields and a reflex autopilot's plans take — the
//! Claude or Codex login the window's panes run with, or a key the person
//! chose — and the TypeSafe key card's three doors for that key.
use tauri::State;

use crate::AppState;
use crate::ShellStateExt as _;
use crate::api_routers::{Keychain, RouterRefusal};
use crate::computer_use::errand::value::cli_found;
use crate::type_value_keys::{self, GeneratorCard};

fn card_now(state: &AppState) -> Result<GeneratorCard, RouterRefusal> {
    let keychain = Keychain::of_this_machine();
    let keys = type_value_keys::read(&keychain, keychain.keeps_keys())?;
    Ok(type_value_keys::card(
        crate::settings_runtime::computer_generator_road(state.settings()),
        state.config_root(),
        cli_found,
        keys,
    ))
}

/// The road chosen, each login road and whose account it runs as, the last
/// answer, and every key a walk reads with whether one is saved — never the
/// key.
#[tauri::command(async)]
pub(crate) fn type_value_keys(state: State<'_, AppState>) -> Result<GeneratorCard, RouterRefusal> {
    card_now(&state)
}

/// Keep the key named `credential_key` where the next walk reads it.
#[tauri::command(async)]
pub(crate) fn save_type_value_key(
    state: State<'_, AppState>,
    credential_key: String,
    key: String,
) -> Result<GeneratorCard, RouterRefusal> {
    type_value_keys::save(&credential_key, &key, &Keychain::of_this_machine())?;
    card_now(&state)
}

/// Forget the key saved under `credential_key`.
#[tauri::command(async)]
pub(crate) fn remove_type_value_key(
    state: State<'_, AppState>,
    credential_key: String,
) -> Result<GeneratorCard, RouterRefusal> {
    type_value_keys::remove(&credential_key, &Keychain::of_this_machine())?;
    card_now(&state)
}
