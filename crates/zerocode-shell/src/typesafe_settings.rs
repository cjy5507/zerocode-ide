//! TypeSafe (Jev) in the window's settings: the key zo's decision shadow asks
//! System One with, and whether the shadow runs.
//!
//! The key is one keychain item — `dev.zerocode.key.TYPESAFE_API_KEY`, both
//! halves spelled by `zerocode_harness` — and no launch hands it to anyone:
//! every zo, started by this window or typed into a shell, reads that item
//! itself when it first needs the key (`api::find_service_keys_in_keychain`),
//! so nothing zo spawns inherits it. The switch is `smart.decisionShadow` in
//! zo's global settings file, the key zo's router reads; nothing else of that
//! file moves. Whether a saved key works is zo's answer, not this window's:
//! the check execs `zo decision-shadow check --json`, which puts the shadow's
//! own question through the same key road, request and answer check.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use zerocode_core::jev::{BROWSER, JevMode, JevUse, ROUTING, SMART_SETTINGS_KEY};
use zerocode_harness::{SERVICE_KEYCHAIN_SERVICE_PREFIX, TYPESAFE_API_KEY_ENV};

use crate::api_routers::{RouterKeys, RouterRefusal};

/// The zo command line that checks the saved key (`zo decision-shadow check`).
pub const ZO_KEY_CHECK_ARGS: [&str; 3] = ["decision-shadow", "check", "--json"];

/// The keychain item the key lives in.
#[must_use]
pub fn typesafe_keychain_service() -> String {
    format!("{SERVICE_KEYCHAIN_SERVICE_PREFIX}{TYPESAFE_API_KEY_ENV}")
}

/// One mode a switch offers, as the pane lists it: the word it writes and what
/// the mode does. The pane words a choice by what it does, so no mode word is
/// spelled anywhere but the Jev use table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModeChoice {
    pub mode: &'static str,
    pub asks: bool,
    pub applies: bool,
    pub automatic: bool,
}

impl ModeChoice {
    fn of(mode: JevMode) -> Self {
        Self {
            mode: mode.key(),
            asks: mode.asks(),
            applies: mode.applies(),
            automatic: mode.automatic(),
        }
    }
}

/// What the pane paints: whether a key could be kept here, whether one is
/// saved — never the key itself — and each switch as its reader reads it, with
/// the modes it offers in the table's order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeSafeSettings {
    pub keys_kept_here: bool,
    pub key_saved: bool,
    pub decision_shadow: &'static str,
    pub decision_modes: Vec<ModeChoice>,
    /// The window's own switch: whether a stopped browser walk may ask which
    /// control to press — the use table's browser row, the same ladder as the
    /// router's, because a person reads them on one card.
    pub browser_action: &'static str,
    pub browser_modes: Vec<ModeChoice>,
}

/// The pane's state, read from the keychain and zo's settings file.
///
/// # Errors
/// An unreadable keychain or settings file.
pub fn read_settings(
    path: &Path,
    keys: &dyn RouterKeys,
    keys_kept_here: bool,
) -> Result<TypeSafeSettings, RouterRefusal> {
    let key_saved = keys
        .read(&typesafe_keychain_service())?
        .is_some_and(|key| !key.trim().is_empty());
    let root = crate::api_routers::read_zo_settings_root(path)?;
    Ok(TypeSafeSettings {
        keys_kept_here,
        key_saved,
        decision_shadow: mode_in(&root, &ROUTING).key(),
        decision_modes: choices(&ROUTING),
        browser_action: mode_in(&root, &BROWSER).key(),
        browser_modes: choices(&BROWSER),
    })
}

/// A use's switch under `smart`, read by the use's own row — the parser zo's
/// router and the window's walk call too: a word the row offers, trimmed and in
/// any case; a typo never starts sending a task or a screen anywhere.
fn mode_in(root: &Map<String, Value>, row: &JevUse) -> JevMode {
    row.mode_of(
        root.get(SMART_SETTINGS_KEY)
            .and_then(|smart| smart.get(row.setting)),
    )
}

/// The modes a use's switch offers, in the table's order.
fn choices(row: &JevUse) -> Vec<ModeChoice> {
    row.modes.iter().copied().map(ModeChoice::of).collect()
}

/// Keep `key` in the keychain item zo reads, trimmed. An empty key is refused
/// rather than saved as "configured, empty".
///
/// # Errors
/// An empty key, a machine with no keychain, or a refused keychain write.
pub fn save_key(key: &str, keys: &dyn RouterKeys) -> Result<(), RouterRefusal> {
    let key = key.trim();
    if key.is_empty() {
        return Err("TypeSafe API 키가 비어 있습니다".to_string().into());
    }
    keys.write(&typesafe_keychain_service(), key)
}

/// Forget the saved key. Nothing saved is not an error.
///
/// # Errors
/// A refused keychain deletion.
pub fn remove_key(keys: &dyn RouterKeys) -> Result<(), RouterRefusal> {
    keys.delete(&typesafe_keychain_service())
}

/// Set `smart.decisionShadow` in zo's settings file to one of the modes the
/// routing row offers, leaving every other key as it stood.
///
/// # Errors
/// A word that is not one of the row's modes, a `smart` value that is not an
/// object (refused rather than overwritten), or an unreadable or unwritable file.
pub fn set_decision_shadow(path: &Path, mode: &str) -> Result<(), String> {
    set_mode(path, &ROUTING, mode)
}

/// Set the window's browser recovery to one of the modes the browser row offers.
///
/// # Errors
/// The same three as [`set_decision_shadow`].
pub fn set_browser_action(path: &Path, mode: &str) -> Result<(), String> {
    set_mode(path, &BROWSER, mode)
}

/// Write one of a row's own words to its switch under `smart`.
fn set_mode(path: &Path, row: &JevUse, mode: &str) -> Result<(), String> {
    let word = row
        .offered(mode)
        .ok_or_else(|| format!("알 수 없는 판단 모드입니다: {mode}"))?
        .key();
    crate::api_routers::update_zo_settings_root(path, |root| {
        let smart = root
            .entry(SMART_SETTINGS_KEY)
            .or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(smart) = smart else {
            return Err(format!(
                "settings.json의 {SMART_SETTINGS_KEY}가 JSON 객체가 아니라 바꾸지 않았습니다"
            ));
        };
        smart.insert(row.setting.to_string(), Value::String(word.to_string()));
        Ok(())
    })
}

/// What `zo decision-shadow check --json` said: the model that answered and
/// how long it took, or the failure token of why none did (`no_key`,
/// `unauthorized`, `timeout`, …).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeSafeCheck {
    pub answered: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    pub elapsed_ms: u64,
}

/// The check's answer from zo's stdout — the one JSON object it prints, whether
/// or not anything answered (zo exits 1 when nothing did). `None` when stdout
/// holds no such object: a zo too old to know the verb, or one that crashed.
#[must_use]
pub fn read_check(stdout: &[u8]) -> Option<TypeSafeCheck> {
    String::from_utf8_lossy(stdout)
        .lines()
        .find_map(|line| serde_json::from_str::<TypeSafeCheck>(line.trim()).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api_routers::{HeldKeys, Keychain, RouterRefusalKind};

    #[test]
    fn the_key_lives_in_the_item_zo_reads() {
        assert_eq!(
            typesafe_keychain_service(),
            "dev.zerocode.key.TYPESAFE_API_KEY"
        );
    }

    /// A saved key is kept trimmed under the item zo reads, reported as saved
    /// and never echoed; an empty one is refused, and a removal forgets it.
    #[test]
    fn a_key_is_saved_trimmed_reported_without_itself_and_removed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let keys = HeldKeys::default();

        assert!(
            !read_settings(&path, &keys, true)
                .expect("settings")
                .key_saved
        );
        assert!(save_key("   ", &keys).is_err(), "an empty key is refused");
        assert_eq!(keys.held(&typesafe_keychain_service()), None);

        save_key("  apikey_test  ", &keys).expect("save");
        assert_eq!(
            keys.held(&typesafe_keychain_service()).as_deref(),
            Some("apikey_test")
        );
        let settings = read_settings(&path, &keys, true).expect("settings");
        assert!(settings.key_saved);
        let wire = serde_json::to_string(&settings).expect("serializes");
        assert!(
            !wire.contains("apikey_test"),
            "the pane is never sent the key: {wire}"
        );

        remove_key(&keys).expect("remove");
        assert_eq!(keys.held(&typesafe_keychain_service()), None);
        assert!(
            !read_settings(&path, &keys, true)
                .expect("settings")
                .key_saved
        );
    }

    /// Off macOS there is no keychain: the save is refused in the word the
    /// pane translates, rather than "kept" nowhere.
    #[test]
    fn a_machine_with_no_keychain_refuses_the_key_by_kind() {
        let refusal = save_key("apikey_test", &Keychain::without_a_store()).expect_err("refused");
        assert_eq!(refusal.kind, RouterRefusalKind::KeychainUnavailable);
    }

    /// Each switch is written as its reader reads it and nothing else of the
    /// file moves: the router rows, other `smart` knobs and unknown keys stay as
    /// they were. Every mode a row offers goes in and reads back as itself.
    #[test]
    fn the_switch_is_written_where_zo_reads_it_and_nothing_else_moves() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let before = serde_json::json!({
            "providers": [{"name": "OpenRouter", "base_url": "https://openrouter.ai/api/v1",
                           "models": [], "requires_auth": true, "auth_env": "ZEROCODE_ROUTER_OPENROUTER_KEY"}],
            "smart": {"plan": {"minEdge": 3}},
            "model": "fable",
        });
        std::fs::write(&path, before.to_string()).expect("write");
        let keys = HeldKeys::default();
        let read = || read_settings(&path, &keys, true).expect("settings");
        assert_eq!(read().decision_shadow, JevMode::Off.key());
        assert_eq!(read().browser_action, JevMode::Off.key());

        type Writer = fn(&Path, &str) -> Result<(), String>;
        let switches: [(&JevUse, Writer); 2] = [
            (&ROUTING, set_decision_shadow),
            (&BROWSER, set_browser_action),
        ];
        for (row, write) in switches {
            for mode in row.modes.iter().rev() {
                write(&path, mode.key()).expect("a mode the row offers");
                let after: Value =
                    serde_json::from_str(&std::fs::read_to_string(&path).expect("read"))
                        .expect("json");
                assert_eq!(after[SMART_SETTINGS_KEY][row.setting], mode.key());
                assert_eq!(after["smart"]["plan"], before["smart"]["plan"]);
                assert_eq!(after["providers"], before["providers"]);
                assert_eq!(after["model"], before["model"]);
                let state = read();
                let read_back = if row.id == ROUTING.id {
                    state.decision_shadow
                } else {
                    state.browser_action
                };
                assert_eq!(read_back, mode.key(), "{}", row.id);
            }
            assert!(write(&path, "actual").is_err(), "only the row's words");
        }
        assert!(
            set_browser_action(&path, JevMode::Off.key()).is_ok()
                && read().decision_shadow == JevMode::Off.key(),
            "one switch never moves the other"
        );
    }

    /// Each switch reads as its reader reads it: a mode its row offers, in any
    /// case; a typo or a boolean is off; a `smart` that is not an object is
    /// refused rather than overwritten.
    #[test]
    fn the_switch_reads_as_zo_reads_it_and_refuses_to_overwrite_a_stranger() {
        for row in [&ROUTING, &BROWSER] {
            let read = |value: Value| {
                let root = serde_json::json!({ SMART_SETTINGS_KEY: { row.setting: value } });
                mode_in(root.as_object().expect("object"), row)
            };
            for mode in row.modes {
                let shouted = format!(" {} ", mode.key().to_ascii_uppercase());
                assert_eq!(read(serde_json::json!(shouted)), *mode, "{}", row.id);
            }
            assert_eq!(read(serde_json::json!("shadwo")), JevMode::Off);
            assert_eq!(read(serde_json::json!(true)), JevMode::Off);
        }

        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        std::fs::write(&path, r#"{"smart": "fast"}"#).expect("write");
        assert!(set_decision_shadow(&path, JevMode::Shadow.key()).is_err());
        assert!(set_browser_action(&path, JevMode::On.key()).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).expect("read"),
            r#"{"smart": "fast"}"#,
            "a refused change writes nothing"
        );
    }

    /// zo prints one JSON object whether or not anything answered; anything
    /// else on stdout is no answer at all.
    #[test]
    fn the_check_is_read_from_zos_one_json_line() {
        assert_eq!(
            read_check(
                b"{\"answered\":true,\"elapsedMs\":664,\"model\":\"jev-1.13.0\",\"retries\":0}\n"
            ),
            Some(TypeSafeCheck {
                answered: true,
                model: Some("jev-1.13.0".to_string()),
                failure: None,
                elapsed_ms: 664,
            })
        );
        assert_eq!(
            read_check(b"{\"answered\":false,\"elapsedMs\":515,\"failure\":\"unauthorized\",\"retries\":0}"),
            Some(TypeSafeCheck {
                answered: false,
                model: None,
                failure: Some("unauthorized".to_string()),
                elapsed_ms: 515,
            })
        );
        assert_eq!(read_check(b"unknown argument 'decision-shadow'"), None);
    }

    /// The pane lists each row's modes, in the row's order, from the state it
    /// paints — the page holds no option of its own — asks every command it
    /// paints from, and every command it asks is registered.
    #[test]
    fn the_pane_offers_the_rows_modes_and_asks_registered_commands() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = read_settings(
            &dir.path().join("settings.json"),
            &HeldKeys::default(),
            true,
        )
        .expect("settings");
        for (row, offered) in [
            (&ROUTING, &state.decision_modes),
            (&BROWSER, &state.browser_modes),
        ] {
            let words: Vec<&str> = offered.iter().map(|choice| choice.mode).collect();
            let expected: Vec<&str> = row.modes.iter().map(|mode| mode.key()).collect();
            assert_eq!(
                words, expected,
                "{} offers its row's modes, in order",
                row.id
            );
            for (choice, mode) in offered.iter().zip(row.modes) {
                assert_eq!(
                    (choice.asks, choice.applies, choice.automatic),
                    (mode.asks(), mode.applies(), mode.automatic()),
                    "{} {}",
                    row.id,
                    choice.mode
                );
            }
        }

        let page = include_str!("../../../ui/index.html");
        for select_id in ["typesafe-shadow-select", "typesafe-browser-select"] {
            let select = &page[page
                .find(&format!("id=\"{select_id}\""))
                .expect("the switch")..];
            let select = &select[..select.find("</select>").expect("the switch closes")];
            assert!(
                !select.contains("<option"),
                "{select_id} lists no mode of its own: {select}"
            );
        }

        let main = include_str!("main.rs");
        let handlers = main
            .split_once("tauri::generate_handler![")
            .map(|(_, list)| list)
            .expect("the handler list");
        let pane = include_str!("../../../ui/shell-settings.js");
        for command in [
            "typesafe_settings",
            "save_typesafe_key",
            "remove_typesafe_key",
            "set_decision_shadow",
            "set_browser_action",
            "check_typesafe_key",
        ] {
            assert!(
                handlers.contains(&format!("{command},")),
                "{command} is not registered"
            );
            assert!(
                pane.contains(&format!("invoke(\"{command}\"")),
                "the pane never asks {command}"
            );
        }
    }

    /// The settings harness's fake backend answers both switches with the modes
    /// the table gives them, so the pane is driven by the words and meanings it
    /// will read.
    #[test]
    fn the_settings_harness_mirrors_the_rows() {
        let harness = include_str!("../../../ui/tests/settings.mjs");
        let list = &harness[harness
            .find("const TYPESAFE_DECISION_MODES")
            .expect("the fixture")..];
        let list = &list[..list.find("]);").expect("the fixture closes")];
        let mirrored: Vec<&str> = list
            .lines()
            .map(str::trim)
            .filter(|line| line.contains("mode:"))
            .collect();
        for row in [&ROUTING, &BROWSER] {
            let expected: Vec<String> = row
                .modes
                .iter()
                .map(|mode| {
                    format!(
                        "Object.freeze({{ mode: \"{}\", asks: {}, applies: {}, automatic: {} }}),",
                        mode.key(),
                        mode.asks(),
                        mode.applies(),
                        mode.automatic()
                    )
                })
                .collect();
            assert_eq!(mirrored, expected, "{}", row.id);
        }
    }

    /// A mode word is spelled in the Jev use table and nowhere its readers
    /// live: zo's executors, parser, doctor row and key check, the window's
    /// settings, its commands and its browser recovery, and the pane. A file is
    /// read up to where its tests begin; a file Jev shares with others, only
    /// where Jev is.
    #[test]
    fn the_mode_words_are_spelled_only_in_the_use_table() {
        // Up to the file's test module — not its first test attribute, which
        // can sit on one test-only item above the code that matters.
        fn product(source: &str) -> &str {
            source
                .split("#[cfg(test)]\nmod tests")
                .next()
                .unwrap_or(source)
        }
        fn between<'a>(source: &'a str, from: &str, to: &str) -> &'a str {
            let start = source.find(from).unwrap_or_else(|| panic!("no `{from}`"));
            let rest = &source[start..];
            let end = rest[from.len()..]
                .find(to)
                .unwrap_or_else(|| panic!("nothing ends `{from}` at `{to}`"));
            &rest[..from.len() + end]
        }
        let page = include_str!("../../../ui/index.html");
        let readers = [
            (
                "zo routing judgment",
                product(include_str!(
                    "../../../zo-ide/crates/tools/src/misc_tools/smart_router/decision_shadow.rs"
                )),
            ),
            (
                "zo recall judgment",
                product(include_str!(
                    "../../../zo-ide/crates/tools/src/misc_tools/smart_router/rerank_shadow.rs"
                )),
            ),
            (
                "zo settings",
                between(
                    include_str!(
                        "../../../zo-ide/crates/tools/src/misc_tools/smart_router/settings.rs"
                    ),
                    "/// How a Jev use is set",
                    "/// The plan scorer's knobs",
                ),
            ),
            (
                "zo doctor",
                between(
                    include_str!("../../../zo-ide/crates/zo-ide/src/doctor.rs"),
                    "fn inspect_decision_shadow(",
                    "fn percent(",
                ),
            ),
            (
                "zo key check",
                product(include_str!(
                    "../../../zo-ide/crates/zo-ide/src/decision_shadow_cli.rs"
                )),
            ),
            (
                "window settings",
                product(include_str!("typesafe_settings.rs")),
            ),
            ("window commands", product(include_str!("cmd/typesafe.rs"))),
            (
                "window recovery",
                product(include_str!("computer_use/recover.rs")),
            ),
            (
                "window browser judge",
                product(include_str!("computer_use/recover/live.rs")),
            ),
            ("window wire", product(include_str!("systemone.rs"))),
            (
                "window stall cause",
                product(include_str!("orchestration/stall_cause.rs")),
            ),
            (
                "window walk",
                product(include_str!("computer_use/recover/walk.rs")),
            ),
            (
                "pane script",
                between(
                    include_str!("../../../ui/shell-settings.js"),
                    "/* ---- TypeSafe (Jev) ----",
                    "\n/* ---- ",
                ),
            ),
            (
                "pane routing switch",
                between(page, "id=\"typesafe-shadow-select\"", "</select>"),
            ),
            (
                "pane browser switch",
                between(page, "id=\"typesafe-browser-select\"", "</select>"),
            ),
        ];
        for (reader, source) in readers {
            for mode in JevMode::ALL {
                assert!(
                    !source.contains(&format!("\"{}\"", mode.key())),
                    "{reader} spells the mode word `{}` — read it from zerocode_core::jev",
                    mode.key()
                );
            }
        }
    }

    /// The failures the pane puts into words are tokens zo's check can print.
    #[test]
    fn the_failures_the_pane_words_are_zos_tokens() {
        let zo = include_str!("../../../zo-ide/crates/api/src/systemone.rs");
        let pane = include_str!("../../../ui/shell-settings.js");
        for token in ["unauthorized", "no_key"] {
            assert!(
                zo.contains(&format!("=> \"{token}\"")),
                "zo has no `{token}` failure"
            );
            assert!(
                pane.contains(&format!("token === \"{token}\"")),
                "the pane no longer words `{token}`"
            );
        }
    }

    /// The command line the window execs is the one zo's CLI documents.
    #[test]
    fn the_check_command_line_is_the_one_zo_documents() {
        let zo = include_str!("../../../zo-ide/crates/zo-ide/src/decision_shadow_cli.rs");
        let documented = format!("zo {}", ZO_KEY_CHECK_ARGS[..2].join(" "));
        assert!(
            zo.contains(&format!("{documented} [{}]", ZO_KEY_CHECK_ARGS[2])),
            "zo no longer documents `{documented}`"
        );
    }
}
