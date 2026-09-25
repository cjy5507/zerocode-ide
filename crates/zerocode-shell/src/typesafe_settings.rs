//! TypeSafe (Jev) in the window's settings: the key Jev is asked with, and the
//! one switch a person turns it on and off with (2026-09-23,
//! docs/design/jev-settings-20260917.md §6.1).
//!
//! The key is one keychain item — `dev.zerocode.key.TYPESAFE_API_KEY`, both
//! halves spelled by `zerocode_harness` — and no launch hands it to anyone:
//! every zo, started by this window or typed into a shell, reads that item
//! itself when it first needs the key (`api::find_service_keys_in_keychain`),
//! so nothing zo spawns inherits it. The switch is `smart.jev.enabled` in zo's
//! global settings file, the key both programs' door reads; what a press
//! writes beside it is the core's (`door::switch_on`, `door::switch_off`), and
//! nothing else of that file moves. Whether a saved key works is zo's answer,
//! not this window's:
//! the check execs `zo decision-shadow check --json`, which puts the shadow's
//! own question through the same key road, request and answer check.

use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use zerocode_core::jev::door::{self, JevSettings, NotAnObject};
use zerocode_core::jev::{
    CLASSIFIER_SETTING, ClassifierMode, DEFAULT_MODEL, JEV_USES, JevMode, JevUse, MODEL_SETTING,
    ROUTING, SMART_SETTINGS_KEY, count, model_in, pin_in, pinned_model,
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

/// One feature Jev judges for, as the dashboard paints it: the use's own name
/// — which is also the key its words are looked up under — the settings key
/// a person may write by hand, where it stands now, and the modes it offers,
/// each by what it does.
///
/// There is one of these per row of [`JEV_USES`] and no field named after any
/// single use, so a feature added to the table arrives on the dashboard with
/// it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SwitchRow {
    pub id: &'static str,
    pub setting: &'static str,
    pub mode: &'static str,
    pub modes: Vec<ModeChoice>,
    /// Whether a word a person wrote governs this feature
    /// ([`JevUse::word_in`]): its own (`smart.<setting>`), or — for a feature
    /// split off another, while it has none of its own — the one written for
    /// that other (t-6877, [`JevUse::follows`]). A hand-written choice, which
    /// the switch does not govern until the next press on (§6.1).
    pub written: bool,
    /// How many compared marks the judge wants before the seat's accuracy
    /// may speak ([`JevUse::agreement_rows_wanted`]) — the sample the
    /// dashboard fills its bar toward, and under half of which it says the
    /// sample rather than a share (t-6243 D2/D3). `None` for a seat that
    /// never rises.
    pub agreement_rows_wanted: Option<usize>,
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
    /// Whether the routing seat can be asked under this word (t-6346).
    pub reaches: bool,
}

impl ClassifierChoice {
    fn of(mode: ClassifierMode) -> Self {
        Self {
            mode: mode.key(),
            runs: mode.runs(),
            markers: mode.markers(),
            probes: mode.probes(),
            reaches: mode.reaches(),
        }
    }
}

/// The gate in front of the routing seat, as the pane paints it: the settings
/// key it writes, where it stands, whether where it stands calls the chat
/// probe and whether it reaches the seat at all (every word but `off`,
/// t-6346), and the words it offers.
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
    /// Whether the seat this gate stands in front of can be asked at all.
    pub reaches: bool,
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

/// The one switch (§6.1), as the card and the dashboard paint it: whether Jev
/// is in use here, and where it may send from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JevSwitch {
    /// Something is sent: the door lets a request through
    /// ([`JevSettings::enabled`]), some feature asks, and some folder is
    /// consented. Read off what each program will do rather than off
    /// `smart.jev.enabled` alone, because a machine whose features were set
    /// by hand before the switch existed never wrote it and does send — and
    /// a switch that said otherwise would be one a person cannot trust.
    pub on: bool,
    /// Every folder is consented — the switch's own consent.
    pub everywhere: bool,
    /// Folders consented by name.
    pub folders: usize,
}

impl JevSwitch {
    fn of(document: &Value) -> Self {
        let door = JevSettings::from_root(document);
        let folders = door.folders().count();
        let consented = door.everywhere() || folders > 0;
        let asks = JEV_USES.iter().any(|row| row.mode_in(document).asks());
        Self {
            on: door.enabled && asks && consented,
            everywhere: door.everywhere(),
            folders,
        }
    }
}

/// What the pane paints: whether a key could be kept here, whether one is
/// saved — never the key itself — the one switch, and every feature as its
/// reader reads it, with the modes it offers in the table's order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeSafeSettings {
    pub keys_kept_here: bool,
    pub key_saved: bool,
    /// The one switch: whether Jev is in use here.
    pub jev: JevSwitch,
    /// The model every seat's request names — the pin, or the alias.
    pub model: ModelRow,
    /// Every row of the use table, in the table's order — what the dashboard
    /// reads each feature's standing off.
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
    // Read by the core's own readers, as zo and the door read it: a seat
    // with no word of its own follows the switch (§6.1).
    let document = Value::Object(root.clone());
    Ok(TypeSafeSettings {
        keys_kept_here,
        key_saved,
        jev: JevSwitch::of(&document),
        model: ModelRow::of(&root),
        switches: JEV_USES
            .iter()
            .map(|row| SwitchRow {
                id: row.id,
                setting: row.setting,
                mode: row.mode_in(&document).key(),
                modes: choices(row),
                written: row.word_in(&document).is_some(),
                agreement_rows_wanted: row.agreement_rows_wanted,
            })
            .collect(),
        classifier: ClassifierRow {
            setting: CLASSIFIER_SETTING,
            mode: classifier.key(),
            probes: classifier.probes(),
            reaches: classifier.reaches(),
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

/// Turn Jev on or off (§6.1) in zo's settings file, under the window's
/// settings lock and by one atomic replace, leaving every key the press does
/// not own as it stood. What a press writes is the core's
/// ([`door::switch_on`], [`door::switch_off`]), where the readers of it live.
///
/// # Errors
/// A `smart` or `smart.jev` that is not an object (refused rather than
/// overwritten), or an unreadable or unwritable file.
pub fn set_enabled(path: &Path, on: bool) -> Result<(), String> {
    update_smart(path, |smart| {
        if on {
            door::switch_on(smart)
        } else {
            door::switch_off(smart)
        }
        .map_err(|NotAnObject(key)| {
            format!(
                "settings.json의 {SMART_SETTINGS_KEY}.{key}가 JSON 객체가 아니라 바꾸지 않았습니다"
            )
        })
    })
}

/// Change `smart` in zo's settings file and nothing else — the one door the
/// switch, the classifier and the model pin write through. A missing `smart`
/// is made; one that is not an object is refused rather than overwritten, and
/// a change that refuses writes nothing.
fn update_smart(
    path: &Path,
    change: impl FnOnce(&mut Map<String, Value>) -> Result<(), String>,
) -> Result<(), String> {
    crate::api_routers::update_zo_settings_root(path, |root| {
        let smart = root
            .entry(SMART_SETTINGS_KEY)
            .or_insert_with(|| Value::Object(Map::new()));
        let Value::Object(smart) = smart else {
            return Err(format!(
                "settings.json의 {SMART_SETTINGS_KEY}가 JSON 객체가 아니라 바꾸지 않았습니다"
            ));
        };
        change(smart)
    })
}

/// Set the routing classifier to one of its four words, leaving every other
/// key as it stood.
///
/// Its own door, beside the switch's ([`set_enabled`]), because it is not a
/// feature and writes a different key with a different vocabulary. A word the table does not offer is refused rather than written:
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
        Ok(())
    })
}

/// Pin the model every Jev request names (`smart.jevModel`, t-6187), or
/// unpin it with an empty word, leaving every other key as it stood — and
/// answer the row as the card paints it.
///
/// Its own door, like the classifier's: it is no feature's switch, and its word
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
    update_smart(path, |smart| {
        match pin {
            Some(pin) => {
                smart.insert(MODEL_SETTING.to_string(), Value::String(pin.to_string()));
            }
            None => {
                smart.remove(MODEL_SETTING);
            }
        }
        Ok(())
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

/// How many Jev requests this machine has sent today, and the most one day
/// may send (`smart.jev.dailyRequests`) — `None` when the person set no limit.
/// The dashboard draws it over the table (t-6243 D1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DayBudget {
    pub sent: u64,
    pub most: Option<u64>,
}

/// The day's budget, read where the door reads it (`systemone::Wire::pass`):
/// the count file beside the settings file for the day the window counts in,
/// and the limit by the door's own reader of the settings. A settings file
/// that does not read limits nothing, as it limits nothing at the door.
///
/// Its own read rather than a field of [`TypeSafeSettings`]: the dashboard
/// asks it on every refresh, and the settings answer reads the keychain,
/// which is a `security` process each time.
#[must_use]
pub fn read_day(path: &Path) -> DayBudget {
    let root = crate::api_routers::read_zo_settings_root(path).map_or(Value::Null, Value::Object);
    let sent = path.parent().map_or(0, |home| {
        count::sent(&count::requests_path(home, &crate::systemone::today()))
    });
    DayBudget {
        sent,
        most: JevSettings::from_root(&root).daily_requests,
    }
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

    /// The one switch (§6.1), pressed from the file a person who set every
    /// feature by hand keeps — twenty words, four folders, and the door's own
    /// switch never written, as this machine's file holds it on 2026-09-23.
    /// Before the press the card says Jev is in use in some folders; one press
    /// on and every feature stands at its recommendation in every folder; one
    /// press off and nothing is in use. Nothing the press does not own moves:
    /// the router rows, the model pin, other `smart` knobs, the door's own
    /// neighbours and unknown keys stay as they were.
    #[test]
    fn the_one_switch_is_written_where_both_programs_read_it_and_nothing_else_moves() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let mut smart = serde_json::Map::new();
        for row in &JEV_USES {
            let word = if row.modes.contains(&JevMode::Auto) {
                JevMode::Auto
            } else {
                JevMode::On
            };
            smart.insert(row.setting.to_string(), Value::from(word.key()));
        }
        smart.insert(
            door::JEV_SETTINGS_KEY.to_string(),
            serde_json::json!({
                door::WORKSPACES_SETTING: ["/work/a", "/work/b", "/work/c", "/work/d"],
                "labelDrafts": true,
            }),
        );
        smart.insert(MODEL_SETTING.to_string(), Value::from("jev-1.13.0"));
        smart.insert("plan".to_string(), serde_json::json!({"minEdge": 3}));
        let before = serde_json::json!({
            "providers": [{"name": "OpenRouter", "base_url": "https://openrouter.ai/api/v1",
                           "models": [], "requires_auth": true, "auth_env": "ZEROCODE_ROUTER_OPENROUTER_KEY"}],
            "model": "fable",
            SMART_SETTINGS_KEY: smart,
        });
        std::fs::write(&path, before.to_string()).expect("write");
        let keys = HeldKeys::default();
        let read = || read_settings(&path, &keys, true).expect("settings");
        let file = || -> Value {
            serde_json::from_str(&std::fs::read_to_string(&path).expect("read")).expect("json")
        };
        let untouched = |after: &Value| {
            for key in ["providers", "model"] {
                assert_eq!(after[key], before[key], "{key} moved");
            }
            for key in [MODEL_SETTING, "plan"] {
                assert_eq!(
                    after[SMART_SETTINGS_KEY][key], before[SMART_SETTINGS_KEY][key],
                    "smart.{key} moved"
                );
            }
            assert_eq!(
                after[SMART_SETTINGS_KEY][door::JEV_SETTINGS_KEY]["labelDrafts"],
                true,
                "the door's neighbour moved"
            );
        };

        let held = read();
        assert_eq!(
            held.jev,
            JevSwitch {
                on: true,
                everywhere: false,
                folders: 4,
            },
            "a file set by hand sends from its four folders"
        );
        assert!(held.switches.iter().all(|row| row.written));

        set_enabled(&path, true).expect("on");
        let after = file();
        untouched(&after);
        let held = read();
        assert_eq!(
            held.jev,
            JevSwitch {
                on: true,
                everywhere: true,
                folders: 0,
            }
        );
        for (row, painted) in JEV_USES.iter().zip(&held.switches) {
            assert_eq!(painted.mode, row.recommended.key(), "{}", row.id);
            assert!(!painted.written, "{} kept its own word", row.id);
        }

        set_enabled(&path, false).expect("off");
        let mut expected = after.clone();
        expected[SMART_SETTINGS_KEY][door::JEV_SETTINGS_KEY][door::ENABLED_SETTING] =
            Value::Bool(false);
        assert_eq!(file(), expected, "off moves the switch and nothing else");
        let held = read();
        assert!(!held.jev.on);
        for painted in &held.switches {
            assert_eq!(painted.mode, JevMode::Off.key(), "{}", painted.id);
        }

        // A machine that has never met Jev: nothing in use, and one press on
        // makes the object it needs.
        let fresh = dir.path().join("fresh.json");
        assert!(!read_settings(&fresh, &keys, true).expect("settings").jev.on);
        set_enabled(&fresh, true).expect("on");
        let fresh = read_settings(&fresh, &keys, true).expect("settings");
        assert!(fresh.jev.on && fresh.jev.everywhere);
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
                row.mode_in(&serde_json::json!({ SMART_SETTINGS_KEY: { row.setting: value } }))
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
        for stranger in [r#"{"smart": "fast"}"#, r#"{"smart": {"jev": ["/work"]}}"#] {
            std::fs::write(&path, stranger).expect("write");
            for on in [true, false] {
                assert!(set_enabled(&path, on).is_err(), "{stranger} {on}");
            }
            assert_eq!(
                std::fs::read_to_string(&path).expect("read"),
                stranger,
                "a refused press writes nothing"
            );
        }
    }

    /// A file written before the skill suggestion was a seat of its own
    /// reads as it did before the update (t-6877 round 3, the coordinator's
    /// migration contract m-8181): its person turned every feature off by
    /// hand, the skill search among them, and the suggestion — which read the
    /// search's word until the split — stands off too, governed by that word
    /// and not by the switch, so the card says nothing is sent; a word
    /// written for the suggestion itself is its own, and the search's `off`
    /// stands beside it.
    #[test]
    fn a_file_from_before_the_split_keeps_the_suggestion_at_the_searchs_word() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        let suggestion = &zerocode_core::jev::SKILL_SUGGESTION;
        let search = &zerocode_core::jev::SKILLS;
        let mut smart = serde_json::Map::new();
        for row in JEV_USES.iter().filter(|row| row.id != suggestion.id) {
            smart.insert(row.setting.to_string(), Value::from(JevMode::Off.key()));
        }
        smart.insert(
            door::JEV_SETTINGS_KEY.to_string(),
            serde_json::json!({
                door::ENABLED_SETTING: true,
                door::WORKSPACES_SETTING: [door::EVERY_WORKSPACE],
            }),
        );
        let keys = HeldKeys::default();
        let read = |smart: &serde_json::Map<String, Value>| {
            std::fs::write(
                &path,
                serde_json::json!({ SMART_SETTINGS_KEY: smart }).to_string(),
            )
            .expect("write");
            read_settings(&path, &keys, true).expect("settings")
        };
        let row = |state: &TypeSafeSettings, id: &str| {
            state
                .switches
                .iter()
                .find(|row| row.id == id)
                .map(|row| (row.mode, row.written))
                .expect("a row")
        };

        let before = read(&smart);
        assert_eq!(
            row(&before, suggestion.id),
            (JevMode::Off.key(), true),
            "the suggestion stands at the search's word, which the switch does not govern"
        );
        assert!(
            !before.jev.on,
            "nothing asks, so the switch says nothing is sent"
        );

        smart.insert(
            suggestion.setting.to_string(),
            Value::from(JevMode::Auto.key()),
        );
        let split = read(&smart);
        assert_eq!(row(&split, suggestion.id), (JevMode::Auto.key(), true));
        assert_eq!(row(&split, search.id), (JevMode::Off.key(), true));
        assert!(split.jev.on, "the suggestion asks on its own word");
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
            // The comparison sample the judge wants before accuracy may speak,
            // which the dashboard fills its bar toward (t-6243 D2).
            assert_eq!(
                painted.agreement_rows_wanted, row.agreement_rows_wanted,
                "{}",
                row.id
            );
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

        // The card offers one switch and no choice of mode: its only select
        // is the classifier, folded under 고급 and closed until asked for.
        let page = include_str!("../../../ui/index.html");
        let card = &page[page.find("id=\"typesafe-card\"").expect("the key card")..];
        let card = &card[..card
            .find("<template id=\"jev-features\">")
            .expect("the features follow the card")];
        assert_eq!(card.matches("data-jev-switch ").count(), 1, "one switch");
        assert_eq!(card.matches("<select").count(), 1, "{card}");
        let advanced = &card[card
            .find("<details class=\"settings-fold settings-jev-advanced\"")
            .expect("the advanced fold")..];
        assert!(
            advanced.contains("id=\"route-classifier-select\""),
            "the classifier stands under the advanced fold"
        );
        let opening = &advanced[..advanced.find('>').expect("the fold opens")];
        assert!(
            !opening.contains(" open"),
            "the advanced fold starts closed"
        );
        assert!(
            !page.contains("data-jev-seat"),
            "a feature's mode is chosen on the page"
        );

        let main = include_str!("main.rs");
        let handlers = main
            .split_once("tauri::generate_handler![")
            .map(|(_, list)| list)
            .expect("the handler list");
        // The card's script and the dashboard's: one Jev, two surfaces, and
        // the switch's door (`set_jev_enabled`) and the ledgers' numbers
        // (`jev_summary`) are asked from the file both surfaces share.
        let pane = concat!(
            include_str!("../../../ui/shell-settings.js"),
            include_str!("../../../ui/shell-jev.js")
        );
        assert!(
            !handlers.contains("set_jev_mode,"),
            "a feature's mode is written by the file's own hand, not by a door"
        );
        for command in [
            "typesafe_settings",
            "save_typesafe_key",
            "remove_typesafe_key",
            "set_jev_enabled",
            "set_route_classifier",
            "set_jev_model",
            "check_typesafe_key",
            "jev_summary",
            "jev_day",
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

    /// Every feature names itself in the markup's own language and in each of
    /// the four catalogs — its name, the one-line summary and the paragraph of
    /// what it sends — so no language falls back to another feature's words or
    /// to a bare key. The markup keeps them once, in the use table's order
    /// (`#jev-features`), and a feature added to the table turns this red
    /// until its words exist.
    #[test]
    fn every_feature_speaks_in_every_catalog() {
        let page = include_str!("../../../ui/index.html");
        let i18n = include_str!("../../../ui/shell-i18n.js");
        let template = &page[page
            .find("<template id=\"jev-features\">")
            .expect("the features' words")..];
        let template = &template[..template.find("</template>").expect("the template closes")];
        let named: Vec<&str> = template
            .split("data-jev-feature=\"")
            .skip(1)
            .filter_map(|rest| rest.split('"').next())
            .collect();
        let table: Vec<&str> = JEV_USES.iter().map(|row| row.id).collect();
        assert_eq!(named, table, "the features stand in the table's order");
        for (row, words) in JEV_USES
            .iter()
            .zip(template.split("data-jev-feature=\"").skip(1))
        {
            let key = |part: &str| {
                let marker = format!("data-jev-{part} data-i18n=\"");
                let at = words
                    .find(&marker)
                    .unwrap_or_else(|| panic!("{} has no {part}", row.id));
                words[at + marker.len()..]
                    .split('"')
                    .next()
                    .expect("a key")
                    .to_string()
            };
            let name = key("name");
            assert!(
                name.starts_with("settings.typesafe."),
                "{}: {name} is not a feature key",
                row.id
            );
            for (part, suffix) in [("name", ""), ("summary", "Summary"), ("hint", "Hint")] {
                let said = key(part);
                assert_eq!(said, format!("{name}{suffix}"), "{} {part}", row.id);
                assert_eq!(
                    i18n.matches(&format!("\"{said}\"")).count(),
                    4,
                    "{said} must be in en/ja/zh/es"
                );
            }
        }
    }

    /// What a person reads about Jev — the key card, the card of features and
    /// the dashboard — is said in the product's plain words and not in the
    /// codebase's metaphors (t-6243 D0,
    /// docs/design/jev-dashboard-improvement-plan-20260923.md): a person asks
    /// how often a feature asked, how right it was and what is left before it
    /// applies by itself, and a screen that answers in seats, rows, windows,
    /// thresholds, ledgers, probes, walks and rising is answering a question
    /// only the code asks.
    ///
    /// Read from where the words are written: the markup's Korean under every
    /// `settings.typesafe.*` and `jev.*` key, every Korean literal in the
    /// dashboard's script and in the card's part of the settings script, and
    /// the English catalog under the same keys.
    #[test]
    fn the_jev_surfaces_speak_plain_words_not_the_codebases() {
        /// The metaphors, each as its word and whether a use of it is
        /// recognised anywhere inside a word (`false`) or only as a word of
        /// its own — a noun with its particle, or a counter after a number —
        /// because `행` is also the second syllable of 진행 and `창` the last
        /// of 입력창.
        const KOREAN: [(&str, bool); 11] = [
            ("자리", false),
            ("행", true),
            ("창", true),
            ("문턱", false),
            ("원장", false),
            ("프로브", false),
            ("걷기", false),
            ("오르", false),
            ("오른", true),
            ("물은", true),
            ("답한", true),
        ];
        /// The particles a noun standing alone may carry.
        const PARTICLES: [&str; 22] = [
            "", "은", "는", "이", "가", "을", "를", "의", "만", "도", "에", "과", "와", "으로",
            "로", "마다", "씩", "에서", "까지", "부터", "뿐", "안",
        ];
        /// The uses of a metaphor's word that mean the plain thing, each with
        /// the reason it is plain here.
        const PLAIN_USES: [(&str, &str, &str); 2] = [
            (
                "창",
                "워커 창 배치",
                "the worker's own pane in this app, which is a window",
            ),
            (
                "창",
                "창 제목",
                "a desktop app's window title, sent as it is",
            ),
        ];
        /// The English catalog's metaphors, matched as whole words.
        const ENGLISH: [&str; 14] = [
            "seat", "seats", "ledger", "ledgers", "probe", "probes", "rise", "rises", "rising",
            "risen", "rose", "walk", "walks", "walking",
        ];

        fn hangul(text: &str) -> bool {
            text.chars().any(|c| ('가'..='힣').contains(&c))
        }
        /// The double-quoted and template literals of a script, outside its
        /// comments.
        fn literals(source: &str) -> Vec<String> {
            let mut found = Vec::new();
            let mut chars = source.chars().peekable();
            while let Some(c) = chars.next() {
                match c {
                    '/' if chars.peek() == Some(&'/') => {
                        for next in chars.by_ref() {
                            if next == '\n' {
                                break;
                            }
                        }
                    }
                    '/' if chars.peek() == Some(&'*') => {
                        chars.next();
                        let mut last = ' ';
                        for next in chars.by_ref() {
                            if last == '*' && next == '/' {
                                break;
                            }
                            last = next;
                        }
                    }
                    '"' | '`' | '\'' => {
                        let mut literal = String::new();
                        while let Some(next) = chars.next() {
                            match next {
                                '\\' => {
                                    chars.next();
                                }
                                _ if next == c => break,
                                _ => literal.push(next),
                            }
                        }
                        found.push(literal);
                    }
                    _ => {}
                }
            }
            found
        }
        fn between<'a>(source: &'a str, from: &str, to: &str) -> &'a str {
            let start = source.find(from).unwrap_or_else(|| panic!("no `{from}`"));
            let rest = &source[start..];
            let end = rest[from.len()..]
                .find(to)
                .unwrap_or_else(|| panic!("nothing ends `{from}` at `{to}`"));
            &rest[..from.len() + end]
        }
        fn metaphors_in(text: &str) -> Vec<&'static str> {
            let mut plain = text.to_string();
            for (_, phrase, _) in PLAIN_USES {
                plain = plain.replace(phrase, " ");
            }
            let words: Vec<&str> = plain
                .split(|c: char| !(c.is_alphanumeric() || c == '{' || c == '}'))
                .filter(|word| !word.is_empty())
                .collect();
            KOREAN
                .iter()
                .filter(|(metaphor, alone)| {
                    words.iter().any(|word| {
                        word.match_indices(metaphor).any(|(at, _)| {
                            if !alone {
                                return true;
                            }
                            let before = &word[..at];
                            let after = &word[at + metaphor.len()..];
                            let counted = before
                                .chars()
                                .next_back()
                                .is_some_and(|c| c.is_ascii_digit() || c == '}');
                            (before.is_empty() || counted) && PARTICLES.contains(&after)
                        })
                    })
                })
                .map(|(metaphor, _)| *metaphor)
                .collect()
        }

        let page = include_str!("../../../ui/index.html");
        let mut said: Vec<(String, String)> = Vec::new();
        for prefix in ["data-i18n=\"settings.typesafe.", "data-i18n=\"jev."] {
            let mut rest = page;
            while let Some(at) = rest.find(prefix) {
                rest = &rest[at + "data-i18n=\"".len()..];
                let key = rest.split('"').next().unwrap_or_default().to_string();
                let text = rest
                    .split_once('>')
                    .map(|(_, body)| body.split('<').next().unwrap_or_default())
                    .unwrap_or_default();
                said.push((format!("index.html {key}"), text.trim().to_string()));
            }
        }
        let scripts = [
            ("shell-jev.js", include_str!("../../../ui/shell-jev.js")),
            (
                "shell-settings.js (TypeSafe)",
                between(
                    include_str!("../../../ui/shell-settings.js"),
                    "/* ---- TypeSafe (Jev) ----",
                    "\n/* ---- ",
                ),
            ),
        ];
        for (file, source) in scripts {
            for literal in literals(source) {
                if hangul(&literal) {
                    said.push((file.to_string(), literal));
                }
            }
        }
        let mut offenders: Vec<String> = said
            .iter()
            .flat_map(|(from, text)| {
                metaphors_in(text)
                    .into_iter()
                    .map(move |word| format!("{from}: 「{word}」 in {text}"))
            })
            .collect();

        let i18n = include_str!("../../../ui/shell-i18n.js");
        let english = between(i18n, "\n  en: {\n", "\n  ja: {\n");
        for line in english.lines() {
            let line = line.trim();
            let Some((key, value)) = line
                .strip_prefix('"')
                .and_then(|rest| rest.split_once("\": \""))
            else {
                continue;
            };
            if !(key.starts_with("settings.typesafe.") || key.starts_with("jev.")) {
                continue;
            }
            for word in value
                .split(|c: char| !c.is_ascii_alphabetic())
                .map(str::to_ascii_lowercase)
            {
                if ENGLISH.contains(&word.as_str()) {
                    offenders.push(format!("en {key}: “{word}” in {value}"));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "{} Jev string(s) speak the codebase's metaphors — say what a person \
             means (기능·판단·요청·응답·정확도·표본·기준·자동 적용), or add the \
             plain use to PLAIN_USES with its reason:\n{}",
            offenders.len(),
            offenders.join("\n")
        );
    }

    /// The Jev surfaces' styles speak in tokens (t-6277 D10): every size, ink,
    /// weight and tint on the dashboard and on the settings card's Jev part is
    /// a `var(--…)` from tokens.css, so the two treatments and the contrast the
    /// window suite measures (`contrastTable`) are decided in one place. A
    /// share of a box (0%, 50%, 100%) is geometry, not a token.
    #[test]
    fn the_jev_styles_read_every_size_ink_and_weight_from_tokens() {
        fn block<'a>(css: &'a str, from: &str, to: &str) -> &'a str {
            let start = css.find(from).unwrap_or_else(|| panic!("no `{from}`"));
            let rest = &css[start..];
            &rest[..rest
                .find(to)
                .unwrap_or_else(|| panic!("nothing ends `{from}`"))]
        }
        let css = include_str!("../../../ui/shell.css");
        let blocks = [
            (
                "dashboard",
                block(css, "/* ---- Jev dashboard (t-5807)", ".skills-view {"),
            ),
            (
                "settings card",
                block(
                    css,
                    "/* ---- Jev on the settings card",
                    "/* A row whose mode nothing can reach",
                ),
            ),
        ];
        let mut literals = Vec::new();
        for (surface, text) in blocks {
            let bytes = text.as_bytes();
            let mut at = 0;
            while at < bytes.len() {
                let byte = bytes[at];
                let starts_number = byte.is_ascii_digit()
                    && (at == 0
                        || !(bytes[at - 1].is_ascii_alphanumeric() || bytes[at - 1] == b'-'));
                if starts_number {
                    let end = text[at..]
                        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
                        .map_or(text.len(), |len| at + len);
                    let number = &text[at..end];
                    let unit: String = text[end..]
                        .chars()
                        .take_while(char::is_ascii_alphabetic)
                        .collect();
                    let percent = text[end..].starts_with('%');
                    let geometry = percent && ["0", "50", "100"].contains(&number);
                    if (unit == "px" || percent) && !geometry {
                        literals.push(format!(
                            "{surface}: {number}{}",
                            if percent { "%" } else { "px" }
                        ));
                    }
                    at = end.max(at + 1);
                    continue;
                }
                at += 1;
            }
            for (at, _) in text.match_indices("font-weight:") {
                let value = text[at + "font-weight:".len()..].trim_start();
                if value.starts_with(|c: char| c.is_ascii_digit()) {
                    literals.push(format!(
                        "{surface}: font-weight {}",
                        &value[..3.min(value.len())]
                    ));
                }
            }
            for marker in ["#", "rgb(", "rgba(", "hsl("] {
                for (at, _) in text.match_indices(marker) {
                    let after = &text[at + marker.len()..];
                    let colour =
                        marker != "#" || after.starts_with(|c: char| c.is_ascii_hexdigit());
                    if colour {
                        literals.push(format!(
                            "{surface}: {marker}{}",
                            &after[..6.min(after.len())]
                        ));
                    }
                }
            }
        }
        assert!(
            literals.is_empty(),
            "the Jev styles spell a size, ink or weight of their own — name it in \
             tokens.css's Jev block:\n  {}",
            literals.join("\n  ")
        );
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
                // The setting a feature split off another reads while it has
                // no word of its own (t-6877), named only where there is one.
                let follows = row
                    .follows
                    .map(|setting| format!(", follows: \"{setting}\""))
                    .unwrap_or_default();
                format!(
                    "Object.freeze({{ id: \"{}\", setting: \"{}\", modes: \"{}\", recommended: \"{}\"{follows} }}),",
                    row.id,
                    row.setting,
                    modes.join(" "),
                    row.recommended.key()
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
            // The card's switch and the features' words — where a mode word
            // could stand now that no select offers one (§6.1).
            (
                "pane switch",
                between(page, "data-jev-switch-row", "</label>"),
            ),
            (
                "pane features",
                between(page, "<template id=\"jev-features\">", "</template>"),
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
            // The classifier's part of the card, up to where the advanced
            // fold that holds it closes.
            (
                "pane card",
                between(
                    page,
                    "id=\"route-classifier-card\"",
                    "\n          </details>",
                ),
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

    /// The harness's fake backend presses the switch the way the core does
    /// (§6.1): the door's own object and the every-folder word are the
    /// core's, and the card asks the switch's own door.
    #[test]
    fn the_settings_harness_mirrors_the_switch() {
        let harness = include_str!("../../../ui/tests/settings.mjs");
        assert!(
            harness.contains(&format!(
                "const JEV_SETTINGS_KEY = \"{}\"",
                door::JEV_SETTINGS_KEY
            )),
            "the harness writes the switch under another object than the door reads"
        );
        assert!(
            harness.contains(&format!(
                "const JEV_EVERY_WORKSPACE = \"{}\"",
                door::EVERY_WORKSPACE
            )),
            "the harness's every-folder word is not the core's"
        );
        assert!(harness.contains("case \"set_jev_enabled\""));
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
                    "Object.freeze({{ mode: \"{}\", runs: {}, markers: {}, probes: {}, reaches: {} }}),",
                    mode.key(),
                    mode.runs(),
                    mode.markers(),
                    mode.probes(),
                    mode.reaches()
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
        assert!(untouched.reaches);
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
        assert!(
            quiet.reaches,
            "the provider-free word still reaches the routing seat (t-6346)"
        );
        set_classifier(&path, ClassifierMode::Off.key()).expect("set");
        assert!(
            !read().reaches,
            "with automatic routing off nothing reaches the seat"
        );
        set_classifier(&path, ClassifierMode::Deterministic.key()).expect("set");
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

    /// Every outcome token a feature's week can carry has its words in the
    /// one table the dashboard's chips and the card's key check both read
    /// (`JEV_TOKENS`, t-6243 D5): each of zo's wire failures, and each of the
    /// door's refusals but `off`, which is a mode word the dashboard never
    /// spells. A token zo or the door gains turns this red until it is worded.
    #[test]
    fn the_failures_the_pane_words_are_zos_tokens() {
        fn between<'a>(source: &'a str, from: &str, to: &str) -> &'a str {
            let start = source.find(from).unwrap_or_else(|| panic!("no `{from}`"));
            let rest = &source[start..];
            let end = rest[from.len()..]
                .find(to)
                .unwrap_or_else(|| panic!("nothing ends `{from}` at `{to}`"));
            &rest[..from.len() + end]
        }
        let zo = include_str!("../../../zo-ide/crates/api/src/systemone.rs");
        let wire: Vec<&str> = between(zo, "pub const fn token(self)", "\n    }")
            .lines()
            .filter_map(|line| line.split_once("=> \""))
            .filter_map(|(_, rest)| rest.split('"').next())
            .collect();
        assert!(
            wire.contains(&"unauthorized") && wire.contains(&"no_key"),
            "zo's failure tokens were not read: {wire:?}"
        );
        let door = zerocode_core::jev::door::Refused::ALL
            .into_iter()
            .filter(|refused| *refused != zerocode_core::jev::door::Refused::Off)
            .map(zerocode_core::jev::door::Refused::token);
        let table = between(
            include_str!("../../../ui/shell-jev.js"),
            "const JEV_TOKENS = Object.freeze({",
            "\n});",
        );
        for token in wire.iter().copied().chain(door) {
            assert!(
                table.contains(&format!("\n  {token}: {{")),
                "the dashboard's token table does not word `{token}`"
            );
        }
        // The card's key check says its failures from the same rows.
        let check = between(
            include_str!("../../../ui/shell-settings.js"),
            "function typesafeCheckFailure(",
            "\n}",
        );
        assert!(
            check.contains("jevTokenRow("),
            "the key check words its failures somewhere else"
        );
    }

    /// The day's count the dashboard draws is the door's own: the file the
    /// door counts one byte per request into, for the day the window counts
    /// in, against the limit the door reads — and a day nothing was sent is
    /// zero, a person who set no limit has none (t-6243 D1).
    #[test]
    fn the_day_is_counted_where_the_door_counts_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("settings.json");
        assert_eq!(
            read_day(&path),
            DayBudget {
                sent: 0,
                most: None
            },
            "no file and no setting is nothing sent and no limit"
        );
        let today = count::requests_path(dir.path(), &crate::systemone::today());
        for _ in 0..3 {
            count::count_one(&today).expect("count one");
        }
        std::fs::write(
            &path,
            r#"{"smart":{"jev":{"dailyRequests":500,"workspaces":["/x"]}}}"#,
        )
        .expect("write settings");
        assert_eq!(
            read_day(&path),
            DayBudget {
                sent: 3,
                most: Some(500)
            }
        );
        let drawn = serde_json::to_value(read_day(&path)).expect("serializes");
        assert_eq!(drawn, serde_json::json!({ "sent": 3, "most": 500 }));
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
                     "agreement":{"compared":44,"agreed":16,"lowerBound":0.24,"controlRows":0,
                                  "baselineCompared":44,"baselineAgreed":40,"baselineShare":0.91,"notCompared":3}},
           "baseline":"todays_rule","negativesWanted":3,
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
        // The cheapest reader beside it, and what compared nothing, ride
        // through to the drawer (t-6342).
        assert_eq!(
            (
                judged.agreement.baseline_compared,
                judged.agreement.baseline_agreed,
                judged.agreement.not_compared
            ),
            (44, 40, 3)
        );
        assert_eq!(summon.baseline.as_deref(), Some("todays_rule"));
        assert_eq!(summon.negatives_wanted, Some(3));
        assert_eq!(drawn["judged"]["agreement"]["baselineShare"], 0.91);
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
        assert_eq!((seat.calibration.as_ref(), seat.apply_share), (None, None));
    }

    /// A seat's act line reaches the dashboard as the screen reads it
    /// (t-9468): the line drawn or why none, the table's line, the answers
    /// at the fixed and drawn lines, and at the table's line the three
    /// numbers — while zo's grid and the row the replay keeps stay in zo's
    /// answer.
    #[test]
    fn a_summary_carries_the_act_line_the_screen_draws_and_not_the_grid() {
        let stdout = br#"{"seats":[{"id":"notify","stand":"recording","applies":false,
          "today":{"rows":1,"answered":1},"week":{"rows":1,"answered":1},
          "calibration":{"readsActLine":true,"marksWanted":40,"actFromPermille":300,"reason":null,
            "grid":[{"fromPermille":0,"marks":100}],"row":{"seat":"notify","grid":[]},
            "fixed":{"fromPermille":850,"marks":50,"applyShare":0.5,"errorPermille":60,
                     "baselineErrorPermille":1000,"underErrorPermille":500},
            "drawn":{"fromPermille":300,"marks":50,"applyShare":0.5,"errorPermille":60,
                     "baselineErrorPermille":1000,"underErrorPermille":500},
            "tableLine":300,"atTableLine":{"fromPermille":300}},
          "applyShare":0.5,"appliedErrorPermille":60,"baselineErrorPermille":1000}]}"#;
        let notify = &read_summary(stdout).expect("a summary")[0];
        let calibration = notify.calibration.as_ref().expect("the act line");
        assert_eq!(
            (
                calibration.act_from_permille,
                calibration.table_line,
                calibration.reason.as_deref()
            ),
            (Some(300), Some(300), None)
        );
        assert_eq!(calibration.fixed.map(|at| at.from_permille), Some(850));
        assert_eq!(
            (
                notify.apply_share,
                notify.applied_error_permille,
                notify.baseline_error_permille
            ),
            (Some(0.5), Some(60), Some(1_000))
        );
        let drawn = serde_json::to_value(notify).expect("serializes");
        assert_eq!(drawn["calibration"]["drawn"]["underErrorPermille"], 500);
        assert!(
            drawn["calibration"].get("grid").is_none() && drawn["calibration"].get("row").is_none(),
            "the grid and the row stay in zo's answer: {}",
            drawn["calibration"]
        );
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
                   "failures":[{"token":"no_key","rows":3}],
                   "guards":{"instructed":2,"walled":1},"controls":{"named":7,"destructiveHeld":2}},
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
        // What a screen seat's guards stopped and the controls it handed over
        // (t-6277 D6), as the core counted them.
        assert_eq!(
            routing.week.guards,
            SeatGuards {
                instructed: 2,
                walled: 1
            }
        );
        assert_eq!(
            routing.week.controls,
            SeatControls {
                named: 7,
                destructive_held: 2
            }
        );
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
            (summon.week.guards, summon.week.controls),
            (SeatGuards::default(), SeatControls::default()),
            "a zo older than the count reads as nothing stopped"
        );
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
    /// The cheapest reader the seat is held against, by the table's word
    /// (`always_same`, `todays_rule`, `none`), and how many times its label
    /// must have said no (t-6342) — absent from a zo older than the baseline.
    #[serde(default)]
    pub baseline: Option<String>,
    #[serde(default)]
    pub negatives_wanted: Option<usize>,
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
    /// What the seat's graded answers say of its act line, beside the line
    /// the product reads for it now (t-9468) — absent from a zo older than
    /// the act line, and for a seat that never rises.
    #[serde(default)]
    pub calibration: Option<SeatCalibration>,
    /// At the line the product reads for the seat now: the share of its
    /// answered requests it acts on, how often the marks of what it acts on
    /// say it was wrong, and how often its baseline was on the same marks,
    /// per thousand (t-9468) — absent while no line is read for it.
    #[serde(default)]
    pub apply_share: Option<f64>,
    #[serde(default)]
    pub applied_error_permille: Option<u16>,
    #[serde(default)]
    pub baseline_error_permille: Option<u16>,
    /// The file zo read the seat's rows from — kept to say where they are
    /// kept ([`crate::jev_scope::with_reach`]), never sent on: a path of the
    /// person's disk is not the dashboard's to draw.
    #[serde(default, skip_serializing)]
    pub found: Option<String>,
    /// Where the seat's rows are kept — its project's own, or this machine's
    /// and so the same in every scope (t-9091); absent for a seat with no
    /// rows on disk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<crate::jev_scope::Reach>,
    /// Across every project summed: how many were counted, in how many the
    /// seat acts, and whether its numbers are a sum (t-9091); absent in a
    /// reading of one checkout.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub across: Option<crate::jev_scope::SeatAcross>,
}

/// What a seat's graded answers say of its act line (t-9468), as the
/// dashboard reads it: the line they draw or why none, the line the product
/// reads now, and the answers at the bands' fixed line and at the drawn one.
/// zo's grid and the row the replay keeps stay in zo's answer — the screen
/// draws none of them.
#[derive(Debug, Clone, PartialEq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatCalibration {
    #[serde(default)]
    pub reads_act_line: bool,
    /// The line the graded answers draw, per thousand — or why none
    /// (`reason`: `whole`, `one_colour`, `non_monotone`, `no_lift`,
    /// `no_confidence`, or the judge's own line word).
    #[serde(default)]
    pub act_from_permille: Option<u16>,
    #[serde(default)]
    pub reason: Option<String>,
    /// The line the product reads for the seat now — the table's row.
    #[serde(default)]
    pub table_line: Option<u16>,
    /// The answers at the line the seat's bands fix, and at the drawn one.
    #[serde(default)]
    pub fixed: Option<SeatLine>,
    #[serde(default)]
    pub drawn: Option<SeatLine>,
}

/// A seat's graded answers at one line: the line, how many marks it holds,
/// the share of answered requests it acts on, and how often the marks of
/// what it acts on, the baseline on those marks and the marks of what it
/// leaves alone say wrong, per thousand.
#[derive(Debug, Clone, Copy, PartialEq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatLine {
    pub from_permille: u16,
    #[serde(default)]
    pub marks: usize,
    #[serde(default)]
    pub apply_share: Option<f64>,
    #[serde(default)]
    pub error_permille: Option<u16>,
    #[serde(default)]
    pub baseline_error_permille: Option<u16>,
    #[serde(default)]
    pub under_error_permille: Option<u16>,
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
    /// The seat's cheapest baseline over the same marks, and its share
    /// (t-6342); zeros and `None` from a zo older than the baseline.
    #[serde(default)]
    pub baseline_compared: usize,
    #[serde(default)]
    pub baseline_agreed: usize,
    #[serde(default)]
    pub baseline_share: Option<f64>,
    /// Label rows that compared nothing and said why.
    #[serde(default)]
    pub not_compared: usize,
    /// Why, word by word — the writers' own words with their counts
    /// (t-9556); empty from a zo older than the words.
    #[serde(default)]
    pub not_compared_by: std::collections::BTreeMap<String, usize>,
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
    /// What a screen seat's guards stopped (t-6187), counted by the core
    /// (`summary::Guards`); zeros from a zo older than the count.
    #[serde(default)]
    pub guards: SeatGuards,
    /// The controls a screen seat's rows named, and the ones it handed to the
    /// person (`summary::Controls`).
    #[serde(default)]
    pub controls: SeatControls,
}

/// Presses a screen seat's two guards stopped: the screen's own text told an
/// assistant what to do, or the screen was a wall in front of the page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatGuards {
    #[serde(default)]
    pub instructed: usize,
    #[serde(default)]
    pub walled: usize,
}

/// The controls a screen seat's rows named, and the ones a press cannot take
/// back that it handed to the person instead of pressing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SeatControls {
    #[serde(default)]
    pub named: usize,
    #[serde(default)]
    pub destructive_held: usize,
}

/// What `zo jev summary --json` said, or `None` when it printed nothing this
/// reader understands — an older zo, or one that failed before it answered.
#[must_use]
pub fn read_summary(stdout: &[u8]) -> Option<Vec<SeatNumbers>> {
    let value: Value = serde_json::from_slice(stdout).ok()?;
    serde_json::from_value(value.get("seats")?.clone()).ok()
}
