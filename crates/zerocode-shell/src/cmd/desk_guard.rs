//! zo's desk guard, shown and turned in the window's settings (v1.1.146).
//!
//! zo refuses a shell command of the model's that moves the person's screen
//! (`zo-ide/crates/runtime/src/desk_guard.rs`, t-104888) while the person's
//! settings leave `deskGuard` on — a key zo takes from the person's own user
//! settings alone, never from a repository's file, a workspace's or a
//! `--settings` file. Until now the one way to turn it was to edit that file
//! by hand. The Computer Use settings read it and write it here.
//!
//! The write is the person's alone (`PERSON_ONLY_COMMANDS`): a switch that
//! takes a railing down is not an agent's to press. Nothing else in the
//! window names the key or this module's writer — the source contract
//! `desk_guard` holds that.
//!
//! The file is the one every window writer of zo's settings uses
//! ([`api_routers::zo_settings_path`]), which is the one that wins zo's
//! merge. Both answers are written as they are — `false` for off, `true` for
//! on — so what the person pressed is what zo reads even where a lower home
//! of zo's (`ZO_HOME` under a `ZO_CONFIG_HOME`, the legacy home) says
//! otherwise. A key written by hand in such a lower home while this file is
//! silent is not shown here: the window reads the one file it writes.

use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::api_routers;

/// zo's settings key for the guard: zo's `desk_guard::SETTING`, held to the
/// same letters by this module's test.
pub(crate) const DESK_GUARD_KEY: &str = "deskGuard";

/// Why the settings cannot say whether the guard stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Untold {
    /// The settings file has no place, or does not read as settings.
    Unreadable,
    /// The key holds something that is neither `true` nor `false`; zo itself
    /// stops at such a file and names the key.
    NotBoolean,
}

/// What the person's zo settings say about the desk guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub(crate) struct DeskGuardSetting {
    /// Whether a zo session started now would stand the guard; `None` when
    /// the settings cannot say — never a guess.
    on: Option<bool>,
    /// Why they cannot say.
    why: Option<Untold>,
}

impl DeskGuardSetting {
    const fn told(on: bool) -> Self {
        Self {
            on: Some(on),
            why: None,
        }
    }

    const fn untold(why: Untold) -> Self {
        Self {
            on: None,
            why: Some(why),
        }
    }
}

/// What the settings file at `path` says, read as zo reads the key: absent —
/// the file or the key — is on, a boolean is itself, anything else is not an
/// answer.
fn setting_at(path: Option<&Path>) -> DeskGuardSetting {
    let Some(root) = path.and_then(|path| api_routers::read_zo_settings_root(path).ok()) else {
        return DeskGuardSetting::untold(Untold::Unreadable);
    };
    match root.get(DESK_GUARD_KEY) {
        None => DeskGuardSetting::told(true),
        Some(Value::Bool(on)) => DeskGuardSetting::told(*on),
        Some(_) => DeskGuardSetting::untold(Untold::NotBoolean),
    }
}

/// Write the guard's answer into the settings file at `path`, every other key
/// as it stood, and say what the file says afterwards. A file that already
/// says `on` is not written again; one that does not read is left as it is
/// and refused.
fn turn_at(path: Option<&Path>, on: bool) -> Result<DeskGuardSetting, String> {
    let path = path.ok_or_else(|| "설정 경로를 찾을 수 없습니다".to_string())?;
    api_routers::update_zo_settings_root_if_changed(path, |root| {
        let said = Value::Bool(on);
        if root.get(DESK_GUARD_KEY) == Some(&said) {
            return Ok(false);
        }
        root.insert(DESK_GUARD_KEY.to_string(), said);
        Ok(true)
    })?;
    Ok(setting_at(Some(path)))
}

/// What the person's zo settings say about the desk guard. A read: asked
/// when the Computer Use settings are opened, and when the window comes back
/// to the front with them open.
#[tauri::command]
pub(crate) async fn desk_guard_setting() -> Result<DeskGuardSetting, String> {
    tauri::async_runtime::spawn_blocking(|| setting_at(api_routers::zo_settings_path().as_deref()))
        .await
        .map_err(|error| error.to_string())
}

/// Turn zo's desk guard on or off in the person's own zo settings file. zo
/// reads the key once in a process, the first time it catches such a
/// command: a session started afterwards takes the new answer, one already
/// running may keep the one it read. The person's alone
/// (`PERSON_ONLY_COMMANDS`): turned off, zo no longer refuses a shell command
/// that moves the person's screen.
#[tauri::command]
pub(crate) async fn set_desk_guard(on: bool) -> Result<DeskGuardSetting, String> {
    tauri::async_runtime::spawn_blocking(move || {
        turn_at(api_routers::zo_settings_path().as_deref(), on)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::computer_use::self_surface::{PERSON_ONLY_COMMANDS, person_only_refusal};

    /// A zo home that does not exist yet, and its settings file.
    fn home() -> (tempfile::TempDir, PathBuf) {
        let home = tempfile::tempdir().expect("a home");
        let settings = home.path().join(".zo").join("settings.json");
        (home, settings)
    }

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().expect("a folder")).expect("the folder");
        std::fs::write(path, text).expect("the file");
    }

    fn document(path: &Path) -> Value {
        serde_json::from_str(&std::fs::read_to_string(path).expect("the file reads"))
            .expect("the file is JSON")
    }

    #[test]
    fn the_key_is_the_one_zo_reads() {
        let zo = include_str!("../../../../zo-ide/crates/runtime/src/desk_guard.rs");
        assert!(
            zo.contains(&format!("pub const SETTING: &str = \"{DESK_GUARD_KEY}\";")),
            "zo's desk guard no longer reads the key the window writes"
        );
    }

    #[test]
    fn settings_that_do_not_name_the_key_leave_the_guard_on() {
        let (_home, settings) = home();
        assert_eq!(setting_at(Some(&settings)), DeskGuardSetting::told(true));
        for text in ["", "{}", r#"{"model": "acme-large", "providers": []}"#] {
            write(&settings, text);
            assert_eq!(
                setting_at(Some(&settings)),
                DeskGuardSetting::told(true),
                "{text}"
            );
        }
    }

    #[test]
    fn the_file_says_off_and_on() {
        let (_home, settings) = home();
        write(&settings, r#"{"deskGuard": false}"#);
        assert_eq!(setting_at(Some(&settings)), DeskGuardSetting::told(false));
        write(&settings, r#"{"deskGuard": true}"#);
        assert_eq!(setting_at(Some(&settings)), DeskGuardSetting::told(true));
    }

    #[test]
    fn a_value_that_is_no_boolean_or_a_file_that_does_not_read_is_never_guessed() {
        let (_home, settings) = home();
        for value in [r#""off""#, "0", "null", "[]", "{}"] {
            write(&settings, &format!(r#"{{"deskGuard": {value}}}"#));
            assert_eq!(
                setting_at(Some(&settings)),
                DeskGuardSetting::untold(Untold::NotBoolean),
                "{value}"
            );
        }
        for text in ["{", "[]", r#"{"providers": {}, "deskGuard": false}"#] {
            write(&settings, text);
            assert_eq!(
                setting_at(Some(&settings)),
                DeskGuardSetting::untold(Untold::Unreadable),
                "{text}"
            );
        }
        assert_eq!(
            setting_at(None),
            DeskGuardSetting::untold(Untold::Unreadable)
        );
        assert_eq!(
            serde_json::to_value(setting_at(None)).expect("a report"),
            json!({ "on": null, "why": "unreadable" })
        );
        assert_eq!(
            serde_json::to_value(DeskGuardSetting::untold(Untold::NotBoolean)).expect("a report"),
            json!({ "on": null, "why": "not_boolean" })
        );
        assert_eq!(
            serde_json::to_value(DeskGuardSetting::told(false)).expect("a report"),
            json!({ "on": false, "why": null })
        );
    }

    #[test]
    fn turning_it_writes_the_one_key_and_keeps_the_rest() {
        let (_home, settings) = home();
        let before = json!({
            "model": "acme-large",
            "providers": [{ "name": "acme", "baseUrl": "https://api.example.com" }],
            "permissions": { "allow": ["bash(cliclick:*)"] },
        });
        write(
            &settings,
            &serde_json::to_string(&before).expect("settings"),
        );

        assert_eq!(
            turn_at(Some(&settings), false),
            Ok(DeskGuardSetting::told(false))
        );
        let mut off = before.clone();
        off["deskGuard"] = json!(false);
        assert_eq!(document(&settings), off);

        // On is written too: this file wins zo's merge, so a lower home of
        // zo's that says off no longer decides.
        assert_eq!(
            turn_at(Some(&settings), true),
            Ok(DeskGuardSetting::told(true))
        );
        let mut on = before;
        on["deskGuard"] = json!(true);
        assert_eq!(document(&settings), on);
    }

    #[test]
    fn turning_it_off_where_no_settings_stand_makes_the_file() {
        let (_home, settings) = home();
        assert!(!settings.exists());
        assert_eq!(
            turn_at(Some(&settings), false),
            Ok(DeskGuardSetting::told(false))
        );
        assert_eq!(document(&settings), json!({ "deskGuard": false }));
    }

    #[test]
    fn a_file_that_already_says_so_is_not_written_again() {
        let (_home, settings) = home();
        let text = "{\"deskGuard\":false,   \"model\":\"acme-large\"}";
        write(&settings, text);
        assert_eq!(
            turn_at(Some(&settings), false),
            Ok(DeskGuardSetting::told(false))
        );
        assert_eq!(
            std::fs::read_to_string(&settings).expect("the file reads"),
            text,
            "the person's own spacing stands where nothing changed"
        );
    }

    #[test]
    fn a_file_that_does_not_read_is_refused_and_left_as_it_is() {
        let (_home, settings) = home();
        let text = "{ \"deskGuard\": false,";
        write(&settings, text);
        assert!(turn_at(Some(&settings), true).is_err());
        assert_eq!(
            std::fs::read_to_string(&settings).expect("the file reads"),
            text
        );
        assert!(turn_at(None, false).is_err());
    }

    /// A value that is no boolean is the person's typing slip in their own
    /// file; their press of the switch — had by a hand that reaches the
    /// command — puts an answer in its place.
    #[test]
    fn a_press_puts_an_answer_where_the_value_was_no_boolean() {
        let (_home, settings) = home();
        write(&settings, r#"{"deskGuard": "off"}"#);
        assert_eq!(
            turn_at(Some(&settings), false),
            Ok(DeskGuardSetting::told(false))
        );
        assert_eq!(document(&settings), json!({ "deskGuard": false }));
    }

    /// The window's second wall: the write runs for the person's own hand and
    /// for no other — an agent's press in the window, a call from no event,
    /// the program road, a hand the page never says. The read is not a
    /// person-only command.
    #[test]
    fn only_the_persons_hand_turns_the_guard() {
        assert!(PERSON_ONLY_COMMANDS.contains(&"set_desk_guard"));
        for hand in [
            Some("agent"),
            Some("none"),
            Some("program"),
            Some("someone"),
            Some(""),
            None,
        ] {
            let refused = person_only_refusal("set_desk_guard", hand)
                .unwrap_or_else(|| panic!("set_desk_guard ran for the hand {hand:?}"));
            assert!(refused.contains("set_desk_guard"), "{refused}");
        }
        assert_eq!(person_only_refusal("set_desk_guard", Some("person")), None);
        assert!(!PERSON_ONLY_COMMANDS.contains(&"desk_guard_setting"));
    }
}
