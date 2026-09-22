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
use zerocode_core::jev::{
    CLASSIFIER_SETTING, ClassifierMode, DEFAULT_MODEL, JEV_USES, JevMode, JevUse, MODEL_SETTING,
    ROUTING, SMART_SETTINGS_KEY, jev_use, model_in, pin_in, pinned_model,
};
use zerocode_harness::{SERVICE_KEYCHAIN_SERVICE_PREFIX, TYPESAFE_API_KEY_ENV};

use crate::api_routers::{RouterKeys, RouterRefusal};

/// The zo command line that checks the saved key (`zo decision-shadow check`).
pub const ZO_KEY_CHECK_ARGS: [&str; 3] = ["decision-shadow", "check", "--json"];

/// The zo command line that counts every seat's ledger (`zo jev summary`).
///
/// The window asks rather than counting: these files are zo's to read, the
/// judge that promotes a seat reads them the same way (§4), and a card that
/// counted for itself would be free to disagree with the seat it is drawing.
pub const ZO_JEV_SUMMARY_ARGS: [&str; 3] = ["jev", "summary", "--json"];
/// The flag that hands zo the window's Computer Use sessions folder, where the
/// screen seats (browser, desktop, emulator) append beside each walk's
/// evidence — without it the card read all three as "never asked" with rows
/// on disk (2026-09-20).
pub const ZO_JEV_SUMMARY_SESSIONS_FLAG: &str = "--computer-use";
/// The flag that names the project whose ledgers zo counts. Without it zo
/// counted the window process's own working directory — never a project —
/// and the card said "nothing asked yet" of a routing seat with 28 rows
/// (2026-09-20, the person's screenshot).
pub const ZO_JEV_SUMMARY_CWD_FLAG: &str = "--cwd";
/// The flag that asks zo to list each seat's last requests under its
/// numbers — what the dashboard shows beside the table
/// (docs/design/jev-dashboard-and-perfection-20260921.md §2 (b)). The card
/// never passes it: it draws the numbers, and a list it would not draw is
/// bytes across the exec boundary for nothing.
pub const ZO_JEV_SUMMARY_RECENT_FLAG: &str = "--recent";

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

/// One place Jev sits, as the pane paints it: the use's own name — which is
/// also the switch's element id and the key its words are looked up under —
/// the settings key it writes, where it stands now, and the modes it offers.
///
/// There is one of these per row of [`JEV_USES`] and no field named after any
/// single use, so a seat added to the table arrives on the card with it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchRow {
    pub id: &'static str,
    pub setting: &'static str,
    pub mode: &'static str,
    pub modes: Vec<ModeChoice>,
}

/// One word the routing classifier may hold, as the pane lists it: the word it
/// writes and what that word DOES. The pane words a choice by what it does, so
/// no classifier word is spelled anywhere but the core table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierChoice {
    pub mode: &'static str,
    pub runs: bool,
    pub markers: bool,
    pub probes: bool,
}

impl ClassifierChoice {
    fn of(mode: ClassifierMode) -> Self {
        Self {
            mode: mode.key(),
            runs: mode.runs(),
            markers: mode.markers(),
            probes: mode.probes(),
        }
    }
}

/// The gate in front of the routing seat, as the pane paints it: the settings
/// key it writes, where it stands, whether where it stands reaches a probe at
/// all, and the words it offers.
///
/// It is not a Jev use and has no row in that table — it is zo's own routing
/// setting. It is answered here because it is the one fact the routing row
/// cannot tell the truth without: `smart.decisionShadow` may say the seat
/// applies while this says nothing is ever asked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClassifierRow {
    pub setting: &'static str,
    pub mode: &'static str,
    pub probes: bool,
    /// The seat this gate stands in front of, by that use's own name — so the
    /// card reads which row to warn on rather than carrying a second copy of
    /// the coupling.
    pub gates: &'static str,
    pub modes: Vec<ClassifierChoice>,
}

/// The model every request names, as the pane paints it (t-6187): the
/// settings key it writes, the model the door asks for now, whether a person
/// pinned it, and the alias an unpinned door asks — so the card never spells
/// the alias itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRow {
    pub setting: &'static str,
    pub model: String,
    pub pinned: bool,
    pub alias: &'static str,
}

impl ModelRow {
    fn of(root: &Map<String, Value>) -> Self {
        // Read by the core's own readers, as the door reads it: a value the
        // door would ignore is shown as no pin.
        let root = Value::Object(root.clone());
        Self {
            setting: MODEL_SETTING,
            model: model_in(&root).to_string(),
            pinned: pin_in(&root).is_some(),
            alias: DEFAULT_MODEL,
        }
    }
}

/// What the pane paints: whether a key could be kept here, whether one is
/// saved — never the key itself — and every switch as its reader reads it,
/// with the modes it offers in the table's order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeSafeSettings {
    pub keys_kept_here: bool,
    pub key_saved: bool,
    /// The model every seat's request names — the pin, or the alias.
    pub model: ModelRow,
    /// Every row of the use table, in the table's order — zo's two and the
    /// window's three on one card, because a person reads them together and
    /// the question each asks is the same: does anything go to the vendor,
    /// and may it change what the product does.
    pub switches: Vec<SwitchRow>,
    /// The routing classifier, which decides whether the routing seat is asked
    /// anything at all.
    pub classifier: ClassifierRow,
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
    let classifier = classifier_in(&root);
    Ok(TypeSafeSettings {
        keys_kept_here,
        key_saved,
        model: ModelRow::of(&root),
        switches: JEV_USES
            .iter()
            .map(|row| SwitchRow {
                id: row.id,
                setting: row.setting,
                mode: mode_in(&root, row).key(),
                modes: choices(row),
            })
            .collect(),
        classifier: ClassifierRow {
            setting: CLASSIFIER_SETTING,
            mode: classifier.key(),
            probes: classifier.probes(),
            gates: ROUTING.id,
            modes: ClassifierMode::ALL.map(ClassifierChoice::of).to_vec(),
        },
    })
}

/// The classifier's word in a settings document, read by the core table's own
/// parser — including its absence, which is not the same as its default.
fn classifier_in(root: &Map<String, Value>) -> ClassifierMode {
    ClassifierMode::of(
        root.get(SMART_SETTINGS_KEY)
            .and_then(|smart| smart.get(CLASSIFIER_SETTING)),
    )
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

/// Set the switch named `use_id` in zo's settings file to one of the modes
/// that use offers, leaving every other key as it stood.
///
/// One door for every seat, because the pane sends the row's own name back:
/// a use the table does not name is refused rather than written, so a stale
/// card cannot create a switch nothing reads.
///
/// # Errors
/// A use the table does not name, a word that is not one of that use's modes,
/// a `smart` value that is not an object (refused rather than overwritten), or
/// an unreadable or unwritable file.
pub fn set_use_mode(path: &Path, use_id: &str, mode: &str) -> Result<(), String> {
    let row = jev_use(use_id).ok_or_else(|| format!("알 수 없는 판단 자리입니다: {use_id}"))?;
    set_mode(path, row, mode)
}

/// Write one of a row's own words to its switch under `smart`.
fn set_mode(path: &Path, row: &JevUse, mode: &str) -> Result<(), String> {
    let word = row
        .offered(mode)
        .ok_or_else(|| format!("알 수 없는 판단 모드입니다: {mode}"))?
        .key();
    update_smart(path, |smart| {
        smart.insert(row.setting.to_string(), Value::String(word.to_string()));
    })
}

/// Change `smart` in zo's settings file and nothing else — the one door the
/// seat switches, the classifier and the model pin write through. A missing
/// `smart` is made; one that is not an object is refused rather than
/// overwritten.
fn update_smart(path: &Path, change: impl FnOnce(&mut Map<String, Value>)) -> Result<(), String> {
    crate::api_routers::update_zo_settings_root(path, |root| {
        let smart = root
            .entry(SMART_SETTINGS_KEY)
            .or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(smart) = smart else {
            return Err(format!(
                "settings.json의 {SMART_SETTINGS_KEY}가 JSON 객체가 아니라 바꾸지 않았습니다"
            ));
        };
        change(smart);
        Ok(())
    })
}

/// Set the routing classifier to one of its four words, leaving every other
/// key as it stood.
///
/// The seat switches have one door each ([`set_use_mode`]); this is its own,
/// because it is not a seat and writes a different key with a different
/// vocabulary. A word the table does not offer is refused rather than written:
/// zo reads an unknown word as the provider-free verdict, so a typo here would
/// quietly stop the routing seat being asked anything.
///
/// # Errors
/// A word the classifier does not offer, a `smart` value that is not an object,
/// or an unreadable or unwritable file.
pub fn set_classifier(path: &Path, mode: &str) -> Result<(), String> {
    let word = ClassifierMode::offered(mode)
        .ok_or_else(|| format!("알 수 없는 분류기 모드입니다: {mode}"))?
        .key();
    update_smart(path, |smart| {
        smart.insert(
            CLASSIFIER_SETTING.to_string(),
            Value::String(word.to_string()),
        );
    })
}

/// Pin the model every Jev request names (`smart.jevModel`, t-6187), or
/// unpin it with an empty word, leaving every other key as it stood — and
/// answer the row as the card paints it.
///
/// Its own door, like the classifier's: it is no seat's switch, and its word
/// is a model id rather than a mode. Unpinned is no key at all, not the alias
/// written down, so an unpinned file follows the alias wherever the vendor
/// moves it. A word the door would not read as a pin — two words, a control
/// character — is refused rather than written.
///
/// # Errors
/// A pin that is not one word, a `smart` value that is not an object, or an
/// unreadable or unwritable file.
pub fn set_model(path: &Path, word: &str) -> Result<ModelRow, String> {
    let pin = if word.trim().is_empty() {
        None
    } else {
        Some(pinned_model(word).ok_or_else(|| format!("알 수 없는 모델 이름입니다: {word}"))?)
    };
    update_smart(path, |smart| match pin {
        Some(pin) => {
            smart.insert(MODEL_SETTING.to_string(), Value::String(pin.to_string()));
        }
        None => {
            smart.remove(MODEL_SETTING);
        }
    })?;
    Ok(ModelRow::of(&crate::api_routers::read_zo_settings_root(
        path,
    )?))
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
        let seat = |state: &TypeSafeSettings, id: &str| {
            state
                .switches
                .iter()
                .find(|row| row.id == id)
                .unwrap_or_else(|| panic!("the card has no {id} row"))
                .mode
        };
        for row in &JEV_USES {
            assert_eq!(seat(&read(), row.id), JevMode::Off.key(), "{}", row.id);
        }

        for row in &JEV_USES {
            for mode in row.modes.iter().rev() {
                set_use_mode(&path, row.id, mode.key()).expect("a mode the row offers");
                let after: Value =
                    serde_json::from_str(&std::fs::read_to_string(&path).expect("read"))
                        .expect("json");
                assert_eq!(after[SMART_SETTINGS_KEY][row.setting], mode.key());
                assert_eq!(after["smart"]["plan"], before["smart"]["plan"]);
                assert_eq!(after["providers"], before["providers"]);
                assert_eq!(after["model"], before["model"]);
                assert_eq!(seat(&read(), row.id), mode.key(), "{}", row.id);
            }
            assert!(
                set_use_mode(&path, row.id, "actual").is_err(),
                "{} takes only its row's words",
                row.id
            );
        }
        assert!(
            set_use_mode(&path, "a seat the table does not name", JevMode::Off.key()).is_err(),
            "a use the table does not name writes nothing"
        );

        // Every seat now stands at the last word its row offered. Moving one
        // back to off leaves every other exactly where it was.
        let before_move: Vec<&str> = JEV_USES.iter().map(|row| seat(&read(), row.id)).collect();
        set_use_mode(&path, JEV_USES[0].id, JevMode::Off.key()).expect("off");
        for (row, stood) in JEV_USES.iter().zip(&before_move).skip(1) {
            assert_eq!(seat(&read(), row.id), *stood, "one switch moved {}", row.id);
        }
    }

    /// The model pin (`smart.jevModel`, t-6187) is written where both
    /// programs' door reads it and nothing else of the file moves; the card
    /// reads it back as the door does. An empty pin unpins — the key leaves
    /// the file and the alias is asked again — and a pin that is not one word
    /// is refused rather than written, because the door would read it as no
    /// pin at all.
    #[test]
    fn the_model_pin_is_written_where_the_door_reads_it_and_an_empty_one_unpins() {
        use zerocode_core::jev::door::JevSettings;
        use zerocode_core::jev::{DEFAULT_MODEL, MODEL_SETTING};
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let before = serde_json::json!({
            "smart": {"plan": {"minEdge": 3}, "decisionShadow": "shadow"},
            "model": "fable",
        });
        std::fs::write(&path, before.to_string()).expect("write");
        let keys = HeldKeys::default();
        let door = || {
            let root: Value =
                serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
            JevSettings::from_root(&root).model
        };
        let card = || read_settings(&path, &keys, true).expect("settings").model;

        assert_eq!(card().model, DEFAULT_MODEL);
        assert!(!card().pinned);
        assert_eq!(card().alias, DEFAULT_MODEL);

        let pinned = set_model(&path, " jev-1.13.0 ").expect("a version is a pin");
        assert_eq!(pinned.model, "jev-1.13.0");
        assert!(pinned.pinned);
        assert_eq!(
            door(),
            "jev-1.13.0",
            "the door reads the pin the card wrote"
        );
        let after: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert_eq!(after["smart"][MODEL_SETTING], "jev-1.13.0");
        assert_eq!(after["smart"]["plan"], before["smart"]["plan"]);
        assert_eq!(after["smart"]["decisionShadow"], "shadow");
        assert_eq!(
            after["model"], "fable",
            "zo's own model setting is not the pin"
        );

        for slip in ["jev 1.13", "jev-1.13.0\nx", "jev\t1"] {
            assert!(set_model(&path, slip).is_err(), "{slip:?} was written");
            assert_eq!(door(), "jev-1.13.0", "a refused pin left the file alone");
        }

        let unpinned = set_model(&path, "  ").expect("an empty pin unpins");
        assert!(!unpinned.pinned);
        assert_eq!(unpinned.model, DEFAULT_MODEL);
        assert_eq!(door(), DEFAULT_MODEL);
        let after: Value =
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json");
        assert!(
            after["smart"].get(MODEL_SETTING).is_none(),
            "unpinned is no key, not the alias written down: {after}"
        );
    }

    /// Each switch reads as its reader reads it: a mode its row offers, in any
    /// case; a typo or a boolean is off; a `smart` that is not an object is
    /// refused rather than overwritten.
    #[test]
    fn the_switch_reads_as_zo_reads_it_and_refuses_to_overwrite_a_stranger() {
        for row in &JEV_USES {
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
        for row in &JEV_USES {
            assert!(
                set_use_mode(&path, row.id, row.modes[1].key()).is_err(),
                "{}",
                row.id
            );
        }
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
        assert_eq!(
            state.switches.len(),
            JEV_USES.len(),
            "the card carries every seat the table names"
        );
        for (row, painted) in JEV_USES.iter().zip(&state.switches) {
            assert_eq!(painted.id, row.id, "the card keeps the table's order");
            assert_eq!(painted.setting, row.setting, "{}", row.id);
            let offered = &painted.modes;
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

        // Every seat has a row on the card, named by the use's own id, and no
        // row lists a mode of its own — the options are built from what the
        // backend answered.
        let page = include_str!("../../../ui/index.html");
        for row in &JEV_USES {
            let select_id = format!("typesafe-{}-select", row.id);
            let select = &page[page
                .find(&format!("id=\"{select_id}\""))
                .unwrap_or_else(|| panic!("the card has no {select_id}"))..];
            let select = &select[..select.find("</select>").expect("the switch closes")];
            assert!(
                !select.contains("<option"),
                "{select_id} lists no mode of its own: {select}"
            );
            assert!(
                select.contains(&format!("data-jev-seat=\"{}\"", row.id)),
                "{select_id} does not say which seat it moves"
            );
        }

        let main = include_str!("main.rs");
        let handlers = main
            .split_once("tauri::generate_handler![")
            .map(|(_, list)| list)
            .expect("the handler list");
        // The card's script and the dashboard's: one Jev, two surfaces, and
        // the seat switch's door (`set_jev_mode`) and the ledgers' numbers
        // (`jev_summary`) are asked from the file both surfaces share.
        let pane = concat!(
            include_str!("../../../ui/shell-settings.js"),
            include_str!("../../../ui/shell-jev.js")
        );
        for command in [
            "typesafe_settings",
            "save_typesafe_key",
            "remove_typesafe_key",
            "set_jev_mode",
            "set_route_classifier",
            "set_jev_model",
            "check_typesafe_key",
            "jev_summary",
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

    /// Every seat's card row names itself in the markup's own language and in
    /// each of the four catalogs — the label, the one-line summary and the
    /// folded paragraph — so no language falls back to another seat's words
    /// or to a bare key. A seat added to the table turns this red until its
    /// row and its words exist.
    #[test]
    fn every_seat_speaks_in_every_catalog() {
        let page = include_str!("../../../ui/index.html");
        let i18n = include_str!("../../../ui/shell-i18n.js");
        for row in &JEV_USES {
            // The seat's row, from its label to its switch: the label's key
            // names the row's words (`settings.typesafe.<word>`), and the
            // summary and the folded paragraph hang off the same word.
            let switch = page
                .find(&format!("data-jev-seat=\"{}\"", row.id))
                .unwrap_or_else(|| panic!("the card has no {} switch", row.id));
            let before = &page[..switch];
            let label = before
                .rfind("class=\"settings-label\" data-i18n=\"")
                .map(|at| &before[at + "class=\"settings-label\" data-i18n=\"".len()..])
                .and_then(|rest| rest.split('"').next())
                .unwrap_or_else(|| panic!("the {} row has no label key", row.id));
            assert!(
                label.starts_with("settings.typesafe."),
                "{}: {label} is not a seat key",
                row.id
            );
            for suffix in ["", "Summary", "Hint"] {
                let key = format!("{label}{suffix}");
                assert!(
                    page.contains(&format!("data-i18n=\"{key}\"")),
                    "the {} row has no {key}",
                    row.id
                );
                assert_eq!(
                    i18n.matches(&format!("\"{key}\"")).count(),
                    4,
                    "{key} must be in en/ja/zh/es"
                );
            }
        }
    }

    /// The settings harness's fake backend answers every seat the table names,
    /// with the settings key it writes and the modes it offers, so the pane is
    /// driven by the words and meanings it will read. A seat added to the table
    /// turns this red until the fixture carries it too.
    #[test]
    fn the_settings_harness_mirrors_the_rows() {
        let harness = include_str!("../../../ui/tests/settings.mjs");
        let list = &harness[harness.find("const JEV_SEATS").expect("the fixture")..];
        let list = &list[..list.find("]);").expect("the fixture closes")];
        let mirrored: Vec<&str> = list
            .lines()
            .map(str::trim)
            .filter(|line| line.contains("id:"))
            .collect();
        let expected: Vec<String> = JEV_USES
            .iter()
            .map(|row| {
                let modes: Vec<&str> = row.modes.iter().map(|mode| mode.key()).collect();
                format!(
                    "Object.freeze({{ id: \"{}\", setting: \"{}\", modes: \"{}\" }}),",
                    row.id,
                    row.setting,
                    modes.join(" ")
                )
            })
            .collect();
        assert_eq!(mirrored, expected);

        // And the meanings behind those words, which the fixture keeps once.
        let meanings = &harness[harness
            .find("const TYPESAFE_DECISION_MODES")
            .expect("the mode list")..];
        let meanings = &meanings[..meanings.find("]);").expect("the mode list closes")];
        let said: Vec<&str> = meanings
            .lines()
            .map(str::trim)
            .filter(|line| line.contains("mode:"))
            .collect();
        let every: Vec<String> = JevMode::ALL
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
        assert_eq!(said, every);
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
                "zo step effort seat",
                product(include_str!(
                    "../../../zo-ide/crates/tools/src/misc_tools/smart_router/step_effort.rs"
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
                "window errand",
                product(include_str!("computer_use/errand.rs")),
            ),
            (
                "window screen judge",
                product(include_str!("computer_use/errand/live.rs")),
            ),
            ("window wire", product(include_str!("systemone.rs"))),
            (
                "window worker room",
                product(include_str!("cmd/worker_room.rs")),
            ),
            (
                "window stall cause",
                product(include_str!("orchestration/stall_cause.rs")),
            ),
            (
                "window browser read",
                product(include_str!("browser_read.rs")),
            ),
            (
                "window walk",
                product(include_str!("computer_use/errand/walk.rs")),
            ),
            (
                "window goal walk",
                product(include_str!("computer_use/errand/desk.rs")),
            ),
            (
                "pane script",
                between(
                    include_str!("../../../ui/shell-settings.js"),
                    "/* ---- TypeSafe (Jev) ----",
                    "\n/* ---- ",
                ),
            ),
            ("dashboard script", include_str!("../../../ui/shell-jev.js")),
        ];
        let switches: Vec<(String, &str)> = JEV_USES
            .iter()
            .map(|row| {
                let id = format!("typesafe-{}-select", row.id);
                let cut = between(page, &format!("id=\"{id}\""), "</select>");
                (format!("pane {} switch", row.id), cut)
            })
            .collect();
        let named = readers
            .iter()
            .map(|(reader, source)| ((*reader).to_string(), *source))
            .chain(switches);
        for (reader, source) in named {
            for mode in JevMode::ALL {
                assert!(
                    !source.contains(&format!("\"{}\"", mode.key())),
                    "{reader} spells the mode word `{}` — read it from zerocode_core::jev",
                    mode.key()
                );
            }
        }
    }

    /// The gate in front of the routing seat is on the card, offers the four
    /// words the core table has, and spells none of them.
    ///
    /// Everything this pane says about the classifier is a claim about zo's
    /// routing, so the claim is held to zo's own source in
    /// `zerocode_core::jev` (`the_routing_seat_is_only_asked_under_the_probing_word`);
    /// what is held HERE is that the card reads that table rather than
    /// carrying a second copy of it.
    #[test]
    fn the_classifier_stands_on_the_card_and_spells_no_word_of_its_own() {
        let page = include_str!("../../../ui/index.html");
        let select = &page[page
            .find("id=\"route-classifier-select\"")
            .expect("the card has no classifier switch")..];
        let select = &select[..select.find("</select>").expect("the switch closes")];
        assert!(
            !select.contains("<option"),
            "the classifier switch lists a word of its own: {select}"
        );
        // The row the gate stands in front of is named by the backend, so the
        // page carries a warning with no seat written into it.
        assert!(
            page.contains("data-jev-unreachable"),
            "no row can say its mode cannot be reached"
        );
        assert!(
            !page.contains("data-jev-unreachable-routing"),
            "the warning names its seat instead of being told which one"
        );

        // Each reader is cut to where the classifier is, because two of these
        // four words are ordinary English elsewhere in the same file and one
        // of them (`off`) is also a Jev mode word, which a different contract
        // above already holds.
        fn between<'a>(source: &'a str, from: &str, to: &str) -> &'a str {
            let start = source.find(from).unwrap_or_else(|| panic!("no `{from}`"));
            let rest = &source[start..];
            let end = rest[from.len()..]
                .find(to)
                .unwrap_or_else(|| panic!("nothing ends `{from}` at `{to}`"));
            &rest[..from.len() + end]
        }
        let script = include_str!("../../../ui/shell-settings.js");
        let readers = [
            (
                "window settings",
                include_str!("typesafe_settings.rs")
                    .split("#[cfg(test)]")
                    .next()
                    .unwrap_or_default(),
            ),
            ("window commands", include_str!("cmd/typesafe.rs")),
            (
                "pane script",
                between(script, "function paintClassifierModes(", "\nasync function"),
            ),
            (
                "pane script gate",
                between(
                    script,
                    "el(\"route-classifier-select\")?.addEventListener",
                    "\n}",
                ),
            ),
            (
                "pane card",
                between(page, "id=\"route-classifier-card\"", "\n        <!--"),
            ),
        ];
        for (reader, source) in readers {
            for mode in ClassifierMode::ALL {
                assert!(
                    !source.contains(&format!("\"{}\"", mode.key())),
                    "{reader} spells the classifier word `{}` — read it from \
                     zerocode_core::jev::ClassifierMode",
                    mode.key()
                );
            }
        }
        assert_eq!(CLASSIFIER_SETTING, "autoClassifier");
    }

    /// The harness's fake backend answers the model pin the way the window
    /// does (t-6187): the core's key and the core's alias, and the card
    /// asks the pin's own door.
    #[test]
    fn the_settings_harness_mirrors_the_model_pin() {
        let harness = include_str!("../../../ui/tests/settings.mjs");
        assert!(
            harness.contains(&format!("const JEV_MODEL_SETTING = \"{MODEL_SETTING}\"")),
            "the harness pins another key than the door reads"
        );
        assert!(
            harness.contains(&format!("const JEV_MODEL_ALIAS = \"{DEFAULT_MODEL}\"")),
            "the harness's alias is not the core's"
        );
        assert!(harness.contains("case \"set_jev_model\""));
    }

    /// The harness's fake backend answers the classifier the way the window
    /// does: the four words, what each one does, and which seat it gates.
    #[test]
    fn the_settings_harness_mirrors_the_classifier() {
        let harness = include_str!("../../../ui/tests/settings.mjs");
        let list = &harness[harness.find("const CLASSIFIER_MODES").expect("the fixture")..];
        let list = &list[..list.find("]);").expect("the fixture closes")];
        let said: Vec<&str> = list
            .lines()
            .map(str::trim)
            .filter(|line| line.contains("mode:"))
            .collect();
        let every: Vec<String> = ClassifierMode::ALL
            .iter()
            .map(|mode| {
                format!(
                    "Object.freeze({{ mode: \"{}\", runs: {}, markers: {}, probes: {} }}),",
                    mode.key(),
                    mode.runs(),
                    mode.markers(),
                    mode.probes()
                )
            })
            .collect();
        assert_eq!(said, every);
        assert!(
            harness.contains(&format!(
                "const CLASSIFIER_SETTING = \"{CLASSIFIER_SETTING}\""
            )),
            "the fixture writes a different settings key"
        );
    }

    /// The card's gate answers the file it reads, the seat it stands in front
    /// of, and refuses a word the setting does not offer.
    #[test]
    fn the_gate_reads_its_file_and_refuses_a_word_zo_would_not_honour() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let keys = HeldKeys::default();
        let read = || {
            read_settings(&path, &keys, true)
                .expect("settings")
                .classifier
        };

        // Nothing written: zo reads that as the probing word, so the card says
        // the seat below is reachable.
        let untouched = read();
        assert_eq!(untouched.setting, CLASSIFIER_SETTING);
        assert_eq!(untouched.mode, ClassifierMode::Probed.key());
        assert!(untouched.probes);
        assert_eq!(
            untouched.gates, ROUTING.id,
            "the gate names the seat it gates"
        );
        assert_eq!(untouched.modes.len(), ClassifierMode::ALL.len());

        // A word this setting does not offer is refused rather than written:
        // zo would read it as the provider-free verdict and the seat would go
        // quiet without anybody being told.
        assert!(set_classifier(&path, "surprise").is_err());
        assert!(set_classifier(&path, "PROBED").is_err());
        assert_eq!(read().mode, ClassifierMode::Probed.key());

        // A written word stands, and nothing else of the file moves.
        std::fs::write(
            &path,
            br#"{"providers":[{"name":"keep"}],"smart":{"decisionShadow":"on"}}"#,
        )
        .expect("seed");
        set_classifier(&path, ClassifierMode::Deterministic.key()).expect("set");
        let quiet = read();
        assert_eq!(quiet.mode, ClassifierMode::Deterministic.key());
        assert!(!quiet.probes, "the provider-free word reaches no probe");
        let root = crate::api_routers::read_zo_settings_root(&path).expect("root");
        assert_eq!(
            root.get("providers")
                .and_then(|rows| rows.as_array())
                .map(Vec::len),
            Some(1),
            "moving the gate moved the router rows"
        );
        assert_eq!(
            read_settings(&path, &keys, true)
                .expect("settings")
                .switches
                .iter()
                .find(|row| row.id == ROUTING.id)
                .map(|row| row.mode),
            Some(JevMode::On.key()),
            "moving the gate moved the seat it gates"
        );
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

    /// The summary flag the window passes is one zo's CLI documents.
    #[test]
    fn the_summary_sessions_flag_is_the_one_zo_documents() {
        let zo = include_str!("../../../zo-ide/crates/zo-ide/src/jev_cli.rs");
        let documented = format!("zo {}", ZO_JEV_SUMMARY_ARGS[..2].join(" "));
        assert!(
            zo.contains(&documented),
            "zo no longer documents `{documented}`"
        );
        assert!(
            zo.contains(&format!("[{ZO_JEV_SUMMARY_SESSIONS_FLAG} <sessions-dir>]")),
            "zo no longer documents `{ZO_JEV_SUMMARY_SESSIONS_FLAG}`"
        );
        assert!(
            zo.contains(&format!("[{ZO_JEV_SUMMARY_CWD_FLAG} <dir>]")),
            "zo no longer documents `{ZO_JEV_SUMMARY_CWD_FLAG}`"
        );
        assert!(
            zo.contains(&format!("[{ZO_JEV_SUMMARY_RECENT_FLAG} <n>]")),
            "zo no longer documents `{ZO_JEV_SUMMARY_RECENT_FLAG}`"
        );
    }

    /// The dashboard's fields — the days, the recent list, the applied count
    /// and the door's refusals — are read when zo sends them and stay empty
    /// from a zo that does not, so the card keeps drawing beside an older zo.
    #[test]
    fn a_summary_carries_the_days_and_the_recent_list_when_zo_sends_them() {
        let stdout = br#"{"windowDays":7,"judgedEveryRows":20,"seats":[
          {"id":"summon","setting":"summonChoice","mode":"on","ledger":"summon-choice.jsonl",
           "found":"/x/summon-choice.jsonl",
           "today":{"rows":2,"answered":2,"refused":0,"applied":0,"refusals":[],"answeredShare":1.0,
                    "answeredLowerBound":0.34,"called":2,"requests":2,"redactedLines":4,"inputTokens":0,
                    "p50Ms":227,"p95Ms":236,"failures":[]},
           "week":{"rows":50,"answered":48,"refused":2,"applied":0,
                   "refusals":[{"token":"not_consented","rows":2}],
                   "answeredShare":0.96,"answeredLowerBound":0.86,"called":48,"requests":48,
                   "redactedLines":100,"inputTokens":0,"p50Ms":230,"p95Ms":410,
                   "failures":[{"token":"not_consented","rows":2}]},
           "costUsd":0.01,"askedModel":"jev-latest","model":"jev-1.13.0",
           "riseFloorPermille":900,"clearsRiseFloor":false,"rowsToNextJudgment":10,
           "judged":{"window":{"rows":34,"answered":34,"answeredShare":1.0,"answeredLowerBound":0.89,
                     "called":34,"requests":34,"redactedLines":0,"inputTokens":0,"p50Ms":230,"p95Ms":410,
                     "failures":[]},"windowWanted":34,
                     "agreement":{"compared":44,"agreed":16,"lowerBound":0.24,"controlRows":0}},
           "stand":"recording","applies":true,
           "verdict":{"verdict":"hold","line":"answered","cutModel":"jev-1.12.0"},
           "days":[{"startMs":1789900000000,"tally":{"rows":3,"answered":3,"answeredShare":1.0,
                    "answeredLowerBound":0.43,"p50Ms":220},"agreement":{"compared":3,"agreed":1}}],
           "recent":[{"at":1790001955550,"outcome":"answered","elapsedMs":227,"cached":false,
                      "asked":{"task":"t-5807","worker":"w-5814","options":5},"answered":"claude",
                      "confidence":0.19,"applied":null,"agreed":true,"followed":null}]}
        ]}"#;
        let seats = read_summary(stdout).expect("a summary");
        let summon = &seats[0];
        // The id asked, the version that answered and the version cut away
        // ride through to the card (t-6187).
        assert_eq!(summon.asked_model.as_deref(), Some("jev-latest"));
        assert_eq!(summon.model.as_deref(), Some("jev-1.13.0"));
        assert_eq!(
            summon
                .verdict
                .as_ref()
                .and_then(|verdict| verdict.cut_model.as_deref()),
            Some("jev-1.12.0")
        );
        let drawn = serde_json::to_value(summon).expect("serializes");
        assert_eq!(drawn["askedModel"], "jev-latest");
        assert_eq!(drawn["verdict"]["cutModel"], "jev-1.12.0");
        assert_eq!(summon.clears_rise_floor, Some(false));
        assert_eq!(summon.week.p50_ms, Some(230));
        assert_eq!(summon.week.called, 48);
        assert_eq!(
            summon.week.refusals,
            vec![SeatFailure {
                token: "not_consented".to_string(),
                rows: 2
            }]
        );
        let judged = summon.judged.as_ref().expect("judged");
        assert_eq!(
            (judged.agreement.compared, judged.agreement.agreed),
            (44, 16)
        );
        assert_eq!(summon.days.len(), 1);
        assert_eq!(summon.days[0].start_ms, 1_789_900_000_000);
        assert_eq!(summon.days[0].tally.rows, 3);
        assert_eq!(summon.days[0].agreement.agreed, 1);
        assert_eq!(
            summon.days[0].tally.p95_ms, None,
            "a day zo left short is read short"
        );
        assert_eq!(summon.recent.len(), 1);
        let one = &summon.recent[0];
        assert_eq!(one.asked.get("task"), Some(&Value::from("t-5807")));
        assert_eq!(one.answered, Value::from("claude"));
        assert_eq!((one.applied, one.agreed), (None, Some(true)));

        // An older zo sends none of it, and the seat reads with the empties.
        let older = br#"{"seats":[{"id":"summon","stand":"recording","applies":false,
          "today":{"rows":0,"answered":0},"week":{"rows":0,"answered":0}}]}"#;
        let seat = &read_summary(older).expect("a summary")[0];
        assert!(seat.days.is_empty() && seat.recent.is_empty());
        assert_eq!(seat.clears_rise_floor, None);
        assert!(seat.week.refusals.is_empty() && seat.week.failures.is_empty());
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

    #[test]
    fn a_summary_is_read_seat_by_seat_and_a_seat_nobody_asked_keeps_its_empties() {
        let stdout = br#"{"windowDays":7,"judgedEveryRows":20,"seats":[
          {"id":"routing","setting":"decisionShadow","mode":"on","ledger":"decision-shadow.jsonl",
           "found":"/x/decision-shadow.jsonl",
           "today":{"rows":1,"answered":1,"answeredShare":1.0,"answeredLowerBound":0.2,"called":1,
                    "requests":1,"redactedLines":2,"inputTokens":9,"p50Ms":616,"p95Ms":616,"failures":[]},
           "week":{"rows":26,"answered":23,"answeredShare":0.8846,"answeredLowerBound":0.7102,"called":26,
                   "requests":26,"redactedLines":30,"inputTokens":90,"p50Ms":616,"p95Ms":4847,
                   "failures":[{"token":"no_key","rows":3}]},
           "costUsd":null,"riseFloorPermille":950,"clearsRiseFloor":false,"rowsToNextJudgment":14,
           "stand":"recording","applies":true,"verdict":{"verdict":"hold","line":"answered"}},
          {"id":"summon","setting":"summonChoice","mode":"auto","ledger":"summon-choice.jsonl",
           "found":null,
           "today":{"rows":0,"answered":0,"answeredShare":null,"answeredLowerBound":null,"called":0,
                    "requests":0,"redactedLines":0,"inputTokens":0,"p50Ms":null,"p95Ms":null,"failures":[]},
           "week":{"rows":0,"answered":0,"answeredShare":null,"answeredLowerBound":null,"called":0,
                   "requests":0,"redactedLines":0,"inputTokens":0,"p50Ms":null,"p95Ms":null,"failures":[]},
           "costUsd":0.0,"riseFloorPermille":null,"clearsRiseFloor":null,"rowsToNextJudgment":null,
           "stand":"recording","applies":false,"verdict":null}
        ]}"#;
        let seats = read_summary(stdout).expect("a summary");
        assert_eq!(seats.len(), 2);

        let routing = &seats[0];
        assert_eq!(routing.id, "routing");
        assert_eq!(routing.week.rows, 26);
        assert_eq!(routing.week.p95_ms, Some(4_847));
        assert_eq!(routing.week.redacted_lines, 30);
        assert_eq!(routing.rise_floor_permille, Some(950));
        assert_eq!(routing.rows_to_next_judgment, Some(14));
        assert!(
            routing.applies,
            "a person's `on` acts whatever the judge says"
        );
        let verdict = routing.verdict.as_ref().expect("a judged seat");
        assert_eq!(
            (verdict.verdict.as_str(), verdict.line.as_deref()),
            ("hold", Some("answered"))
        );

        let summon = &seats[1];
        assert_eq!(
            summon.week.answered_share, None,
            "never asked is not zero percent"
        );
        assert_eq!(summon.week.p95_ms, None);
        assert_eq!(
            summon.verdict, None,
            "a seat that never rises is never judged"
        );
        assert!(!summon.applies);
    }

    #[test]
    fn an_answer_this_reader_does_not_understand_is_no_answer() {
        assert_eq!(read_summary(b""), None);
        assert_eq!(read_summary(b"zo: unknown argument 'jev'"), None);
        assert_eq!(read_summary(br#"{"seats":"soon"}"#), None);
        assert_eq!(
            read_summary(br#"{"windowDays":7}"#),
            None,
            "no seats is not an empty card"
        );
        assert_eq!(read_summary(br#"{"seats":[]}"#), Some(Vec::new()));
    }
}

/// One seat's numbers, as `zo jev summary --json` reports them and the card
/// draws them beside that seat's switch.
///
/// Every field is optional in the answer and stays optional here: a seat
/// nothing has asked yet has no share and no p95, and a card that drew those
/// as zero would be telling a person their seat is failing.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatNumbers {
    pub id: String,
    /// Where the seat stands, and whether it acts right now.
    pub stand: String,
    pub applies: bool,
    #[serde(default)]
    pub verdict: Option<SeatVerdict>,
    #[serde(default)]
    pub rows_to_next_judgment: Option<usize>,
    #[serde(default)]
    pub rise_floor_permille: Option<u16>,
    /// Whether the judged window's lower bound clears the rise line; absent
    /// for a seat that never rises or a window nothing has filled.
    #[serde(default)]
    pub clears_rise_floor: Option<bool>,
    pub today: SeatWindow,
    pub week: SeatWindow,
    /// The window the judge read and the agreement over it, for a seat that
    /// rises — absent from a zo that judged on the week.
    #[serde(default)]
    pub judged: Option<SeatJudged>,
    /// Every `agreed` mark of the week, whether or not the seat rises —
    /// absent from a zo older than the label rows (t-5806).
    #[serde(default)]
    pub agreement_week: Option<SeatAgreement>,
    #[serde(default)]
    pub cost_usd: Option<f64>,
    /// The id the seat asks with — the person's pin or the alias — and the
    /// version the newest answer named (t-6187): the two halves of the line
    /// the card and the dashboard draw under every seat. Absent from a zo
    /// older than the pin, and `model` from a seat nothing answered.
    #[serde(default)]
    pub asked_model: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    /// Today and the days before it, oldest first — the trend the dashboard
    /// draws; empty from a zo that counts only the week.
    #[serde(default)]
    pub days: Vec<SeatDay>,
    /// The seat's last requests, newest first, when the command asked for
    /// them ([`ZO_JEV_SUMMARY_RECENT_FLAG`]); empty otherwise.
    #[serde(default)]
    pub recent: Vec<SeatDecision>,
}

/// One local day of a seat's ledger, counted the way the week is.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatDay {
    pub start_ms: i64,
    pub tally: SeatWindow,
    #[serde(default)]
    pub agreement: SeatAgreement,
}

/// One request of a seat, as zo digested it (`zerocode_core::jev::recent`):
/// the row's own facts, never its body. Passed through as zo shaped it — the
/// dashboard draws whatever facts a seat's rows carry, by their names, and
/// this reader names none of them.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatDecision {
    pub at: i64,
    pub outcome: String,
    #[serde(default)]
    pub elapsed_ms: Option<u64>,
    #[serde(default)]
    pub cached: bool,
    #[serde(default)]
    pub asked: Map<String, Value>,
    #[serde(default)]
    pub answered: Value,
    #[serde(default)]
    pub confidence: Option<f64>,
    #[serde(default)]
    pub applied: Option<bool>,
    #[serde(default)]
    pub agreed: Option<bool>,
    #[serde(default)]
    pub followed: Option<String>,
}

/// One outcome token and how many rows carried it.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatFailure {
    pub token: String,
    pub rows: usize,
}

/// The numbers a seat is promoted on: the last requests its floor can be
/// cleared on, and how often its judgment named what the probe named there.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatJudged {
    pub window: SeatWindow,
    #[serde(default)]
    pub window_wanted: usize,
    #[serde(default)]
    pub agreement: SeatAgreement,
}

/// Comparisons with the probe over the judged window.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatAgreement {
    pub compared: usize,
    pub agreed: usize,
    #[serde(default)]
    pub lower_bound: Option<f64>,
    /// Control rows the comparison borrowed — the routing seat's probe run
    /// once more beside a judgment already acted on; zero elsewhere.
    #[serde(default)]
    pub control_rows: usize,
}

/// What the judge said of a seat's recent window.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatVerdict {
    pub verdict: String,
    #[serde(default)]
    pub line: Option<String>,
    /// The version the window's rows were cut away from at the last change
    /// of version, beside the line it holds on (t-6187).
    #[serde(default)]
    pub cut_model: Option<String>,
}

/// One window of a seat's ledger, counted.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatWindow {
    pub rows: usize,
    pub answered: usize,
    /// Rows the door refused before anything was sent; absent from a zo that
    /// counted them as the seat's own failures.
    #[serde(default)]
    pub refused: usize,
    #[serde(default)]
    pub answered_share: Option<f64>,
    #[serde(default)]
    pub answered_lower_bound: Option<f64>,
    #[serde(default)]
    pub p50_ms: Option<u64>,
    #[serde(default)]
    pub p95_ms: Option<u64>,
    #[serde(default)]
    pub redacted_lines: u64,
    /// Rows whose judgment went over the wire.
    #[serde(default)]
    pub called: usize,
    #[serde(default)]
    pub requests: u64,
    #[serde(default)]
    pub input_tokens: u64,
    /// Rows whose answer is what the product did.
    #[serde(default)]
    pub applied: usize,
    /// Every outcome that was not an answer, by token, most frequent first.
    #[serde(default)]
    pub failures: Vec<SeatFailure>,
    /// The door's refusals among them — no key, not consented, budget — the
    /// rows `refused` counts, one token at a time.
    #[serde(default)]
    pub refusals: Vec<SeatFailure>,
}

/// What `zo jev summary --json` said, or `None` when it printed nothing this
/// reader understands — an older zo, or one that failed before it answered.
#[must_use]
pub fn read_summary(stdout: &[u8]) -> Option<Vec<SeatNumbers>> {
    let value: Value = serde_json::from_slice(stdout).ok()?;
    serde_json::from_value(value.get("seats")?.clone()).ok()
}
