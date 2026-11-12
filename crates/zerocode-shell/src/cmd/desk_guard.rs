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
//! zo merges the settings of its homes — `ZO_CONFIG_HOME`, `ZO_HOME`, `~/.zo`,
//! the legacy home — and the highest that names a key wins. The read follows
//! that order (`settings_files_from`): the first file that names the key is
//! the answer, as zo's own `persons_dream_lessons` reads its key. The write
//! goes to the highest home's file, the one every window writer of zo's
//! settings uses ([`api_routers::zo_settings_path`]), and both answers are
//! written as they are — `false` for off, `true` for on — so what the person
//! pressed is what zo reads even where a lower home says otherwise.

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::Value;

use crate::api_routers;

/// zo's settings key for the guard: zo's `desk_guard::SETTING`, held to the
/// same letters by this module's test.
pub(crate) const DESK_GUARD_KEY: &str = "deskGuard";

/// zo's homes below the person's home folder, the one it reads first first:
/// its own, then the legacy one it still reads (zo's `core_types::paths`,
/// held to the same letters by this module's test).
const HOMES_BELOW_HOME: [&str; 2] = [".zo", ".forge"];

/// The settings file's name in each of zo's homes.
const SETTINGS_FILE: &str = "settings.json";

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

/// The person's zo settings files, the one that wins zo's merge first: the
/// file the window writes ([`api_routers::zo_settings_root_and_path_from`]),
/// then the files of zo's lower homes in zo's own order.
fn settings_files_from(
    config_home: Option<&Path>,
    zo_home: Option<&Path>,
    home: Option<&Path>,
) -> Vec<PathBuf> {
    let Some((_, written)) =
        api_routers::zo_settings_root_and_path_from(config_home, zo_home, home)
    else {
        return Vec::new();
    };
    let mut files = vec![written];
    let below_home = home
        .into_iter()
        .flat_map(|home| HOMES_BELOW_HOME.iter().map(move |name| home.join(name)));
    for root in zo_home.map(Path::to_path_buf).into_iter().chain(below_home) {
        let file = root.join(SETTINGS_FILE);
        if !root.as_os_str().is_empty() && !files.contains(&file) {
            files.push(file);
        }
    }
    files
}

/// [`settings_files_from`] for this window: the homes its environment names,
/// as [`api_routers::zo_settings_path`] reads them.
fn settings_files() -> Vec<PathBuf> {
    settings_files_from(
        std::env::var_os("ZO_CONFIG_HOME").as_deref().map(Path::new),
        std::env::var_os("ZO_HOME").as_deref().map(Path::new),
        dirs::home_dir().as_deref(),
    )
}

/// What `files` say — the person's zo settings files, highest first — read
/// as zo reads the key: the first file that names it is the answer, a
/// boolean is itself, anything else is not an answer, and no file naming it
/// is on. The first file is the one a press writes, so it has to read; a
/// lower one that does not is passed over, as zo's own read of a person's
/// key passes it.
fn setting_in(files: &[PathBuf]) -> DeskGuardSetting {
    let Some((written, lower)) = files.split_first() else {
        return DeskGuardSetting::untold(Untold::Unreadable);
    };
    let Ok(root) = api_routers::read_zo_settings_root(written) else {
        return DeskGuardSetting::untold(Untold::Unreadable);
    };
    let named = root.get(DESK_GUARD_KEY).cloned().or_else(|| {
        lower.iter().find_map(|file| {
            api_routers::read_zo_settings_root(file)
                .ok()?
                .remove(DESK_GUARD_KEY)
        })
    });
    match named {
        None => DeskGuardSetting::told(true),
        Some(Value::Bool(on)) => DeskGuardSetting::told(on),
        Some(_) => DeskGuardSetting::untold(Untold::NotBoolean),
    }
}

/// Write the guard's answer into the first of `files` — the one that wins
/// zo's merge — every other key as it stood, and say what the files say
/// afterwards. A file that already says `on` is not written again; one that
/// does not read is left as it is and refused.
fn turn_in(files: &[PathBuf], on: bool) -> Result<DeskGuardSetting, String> {
    let written = files
        .first()
        .ok_or_else(|| "설정 경로를 찾을 수 없습니다".to_string())?;
    api_routers::update_zo_settings_root_if_changed(written, |root| {
        let said = Value::Bool(on);
        if root.get(DESK_GUARD_KEY) == Some(&said) {
            return Ok(false);
        }
        root.insert(DESK_GUARD_KEY.to_string(), said);
        Ok(true)
    })?;
    Ok(setting_in(files))
}

/// What the person's zo settings say about the desk guard. A read: asked
/// when the Computer Use settings are opened, and when the window comes back
/// to the front with them open.
#[tauri::command]
pub(crate) async fn desk_guard_setting() -> Result<DeskGuardSetting, String> {
    tauri::async_runtime::spawn_blocking(|| setting_in(&settings_files()))
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
    tauri::async_runtime::spawn_blocking(move || turn_in(&settings_files(), on))
        .await
        .map_err(|error| error.to_string())?
}

#[cfg(test)]
mod tests {
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
        assert_eq!(
            setting_in(std::slice::from_ref(&settings)),
            DeskGuardSetting::told(true)
        );
        for text in ["", "{}", r#"{"model": "acme-large", "providers": []}"#] {
            write(&settings, text);
            assert_eq!(
                setting_in(std::slice::from_ref(&settings)),
                DeskGuardSetting::told(true),
                "{text}"
            );
        }
    }

    #[test]
    fn the_file_says_off_and_on() {
        let (_home, settings) = home();
        write(&settings, r#"{"deskGuard": false}"#);
        assert_eq!(
            setting_in(std::slice::from_ref(&settings)),
            DeskGuardSetting::told(false)
        );
        write(&settings, r#"{"deskGuard": true}"#);
        assert_eq!(
            setting_in(std::slice::from_ref(&settings)),
            DeskGuardSetting::told(true)
        );
    }

    #[test]
    fn a_value_that_is_no_boolean_or_a_file_that_does_not_read_is_never_guessed() {
        let (_home, settings) = home();
        for value in [r#""off""#, "0", "null", "[]", "{}"] {
            write(&settings, &format!(r#"{{"deskGuard": {value}}}"#));
            assert_eq!(
                setting_in(std::slice::from_ref(&settings)),
                DeskGuardSetting::untold(Untold::NotBoolean),
                "{value}"
            );
        }
        for text in ["{", "[]", r#"{"providers": {}, "deskGuard": false}"#] {
            write(&settings, text);
            assert_eq!(
                setting_in(std::slice::from_ref(&settings)),
                DeskGuardSetting::untold(Untold::Unreadable),
                "{text}"
            );
        }
        assert_eq!(
            setting_in(&[]),
            DeskGuardSetting::untold(Untold::Unreadable)
        );
        assert_eq!(
            serde_json::to_value(setting_in(&[])).expect("a report"),
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
            turn_in(std::slice::from_ref(&settings), false),
            Ok(DeskGuardSetting::told(false))
        );
        let mut off = before.clone();
        off["deskGuard"] = json!(false);
        assert_eq!(document(&settings), off);

        // On is written too: this file wins zo's merge, so a lower home of
        // zo's that says off no longer decides.
        assert_eq!(
            turn_in(std::slice::from_ref(&settings), true),
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
            turn_in(std::slice::from_ref(&settings), false),
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
            turn_in(std::slice::from_ref(&settings), false),
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
        assert!(turn_in(std::slice::from_ref(&settings), true).is_err());
        assert_eq!(
            std::fs::read_to_string(&settings).expect("the file reads"),
            text
        );
        assert!(turn_in(&[], false).is_err());
    }

    /// A value that is no boolean is the person's typing slip in their own
    /// file; their press of the switch — had by a hand that reaches the
    /// command — puts an answer in its place.
    #[test]
    fn a_press_puts_an_answer_where_the_value_was_no_boolean() {
        let (_home, settings) = home();
        write(&settings, r#"{"deskGuard": "off"}"#);
        assert_eq!(
            turn_in(std::slice::from_ref(&settings), false),
            Ok(DeskGuardSetting::told(false))
        );
        assert_eq!(document(&settings), json!({ "deskGuard": false }));
    }

    #[test]
    fn the_files_are_zos_homes_in_zos_order_and_the_first_is_the_one_the_window_writes() {
        let paths = include_str!("../../../../zo-ide/crates/core-types/src/paths.rs");
        for line in [
            "pub const ZO_CONFIG_HOME_ENV: &str = \"ZO_CONFIG_HOME\";",
            "pub const ZO_HOME_ENV: &str = \"ZO_HOME\";",
            "pub const ZO_DIR_NAME: &str = \".zo\";",
            "const LEGACY_FORGE_DIR_NAME: &str = \".forge\";",
        ] {
            assert!(paths.contains(line), "zo's homes no longer read: {line}");
        }
        let (config, zo, home) = (
            Path::new("/Users/dev/zo-config"),
            Path::new("/Users/dev/zo-home"),
            Path::new("/Users/dev"),
        );
        let file = |root: &str| PathBuf::from(root).join("settings.json");
        for (config_home, zo_home, expected) in [
            (
                None,
                None,
                vec![file("/Users/dev/.zo"), file("/Users/dev/.forge")],
            ),
            (
                Some(config),
                None,
                vec![
                    file("/Users/dev/zo-config"),
                    file("/Users/dev/.zo"),
                    file("/Users/dev/.forge"),
                ],
            ),
            (
                None,
                Some(zo),
                vec![
                    file("/Users/dev/zo-home"),
                    file("/Users/dev/.zo"),
                    file("/Users/dev/.forge"),
                ],
            ),
            (
                Some(config),
                Some(zo),
                vec![
                    file("/Users/dev/zo-config"),
                    file("/Users/dev/zo-home"),
                    file("/Users/dev/.zo"),
                    file("/Users/dev/.forge"),
                ],
            ),
            // A home named twice is read once.
            (
                Some(Path::new("/Users/dev/.zo")),
                Some(Path::new("/Users/dev/.zo")),
                vec![file("/Users/dev/.zo"), file("/Users/dev/.forge")],
            ),
        ] {
            let files = settings_files_from(config_home, zo_home, Some(home));
            assert_eq!(files, expected);
            assert_eq!(
                files.first(),
                api_routers::zo_settings_root_and_path_from(config_home, zo_home, Some(home))
                    .map(|(_, written)| written)
                    .as_ref(),
                "the first file is the one every window writer of zo's settings writes"
            );
        }
        assert_eq!(
            settings_files_from(Some(config), None, None),
            vec![file("/Users/dev/zo-config")]
        );
        assert!(settings_files_from(None, None, None).is_empty());
    }

    /// zo merges its homes, so a lower home's `false` turns the guard off
    /// while the file the window writes is silent: the switch says off, and a
    /// press writes the answer where it wins.
    #[test]
    fn a_lower_home_that_names_the_key_is_the_answer_until_the_written_file_names_it() {
        let (home, low) = home();
        let high = home.path().join("zo-config").join("settings.json");
        let files = [high.clone(), low.clone()];

        write(&low, r#"{"deskGuard": false}"#);
        assert_eq!(setting_in(&files), DeskGuardSetting::told(false));
        write(&high, r#"{"model": "acme-large"}"#);
        assert_eq!(setting_in(&files), DeskGuardSetting::told(false));

        assert_eq!(turn_in(&files, true), Ok(DeskGuardSetting::told(true)));
        assert_eq!(
            document(&high),
            json!({ "model": "acme-large", "deskGuard": true })
        );
        assert_eq!(document(&low), json!({ "deskGuard": false }));

        // The written file's answer stands over the lower one's, whatever it is.
        write(&high, r#"{"deskGuard": false}"#);
        write(&low, r#"{"deskGuard": true}"#);
        assert_eq!(setting_in(&files), DeskGuardSetting::told(false));
        write(&high, r#"{"deskGuard": "off"}"#);
        assert_eq!(
            setting_in(&files),
            DeskGuardSetting::untold(Untold::NotBoolean)
        );

        // A lower file is read only for the key: one that does not read is
        // passed over, one that names something else is not an answer.
        write(&high, "{}");
        write(&low, "{");
        assert_eq!(setting_in(&files), DeskGuardSetting::told(true));
        write(&low, r#"{"deskGuard": 0}"#);
        assert_eq!(
            setting_in(&files),
            DeskGuardSetting::untold(Untold::NotBoolean)
        );

        // The file a press writes has to read, whatever a lower one says.
        write(&high, "{");
        write(&low, r#"{"deskGuard": false}"#);
        assert_eq!(
            setting_in(&files),
            DeskGuardSetting::untold(Untold::Unreadable)
        );
        assert!(turn_in(&files, true).is_err());
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
