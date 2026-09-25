//! The key typing into a page's fields asks with, as the Computer Use pane
//! keeps it (t-9537) — the TypeSafe key card's three doors, for the value
//! seat's keys.
use crate::api_routers::{Keychain, RouterRefusal};
use crate::type_value_keys::{self, ValueKeys};

fn keys_now() -> Result<ValueKeys, RouterRefusal> {
    let keychain = Keychain::of_this_machine();
    type_value_keys::read(&keychain, keychain.keeps_keys())
}

/// Every key a walk reads, and whether one is saved — never the key.
#[tauri::command(async)]
pub(crate) fn type_value_keys() -> Result<ValueKeys, RouterRefusal> {
    keys_now()
}

/// Keep the key named `credential_key` where the next walk reads it.
#[tauri::command(async)]
pub(crate) fn save_type_value_key(
    credential_key: String,
    key: String,
) -> Result<ValueKeys, RouterRefusal> {
    type_value_keys::save(&credential_key, &key, &Keychain::of_this_machine())?;
    keys_now()
}

/// Forget the key saved under `credential_key`.
#[tauri::command(async)]
pub(crate) fn remove_type_value_key(credential_key: String) -> Result<ValueKeys, RouterRefusal> {
    type_value_keys::remove(&credential_key, &Keychain::of_this_machine())?;
    keys_now()
}
