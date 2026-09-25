//! The Computer Use pane's key card (t-9537): the key typing into a page's
//! fields asks with. What a slot names — the key, the rows that read it, the
//! row a walk asks now — is the value seat's table's, handed over by the
//! backend; the window spells none of it, and the backend names the keychain
//! item by the walk's own function, so the card and the walk read one item.

use std::path::Path;

use serde_json::Value;
use zerocode_harness::SERVICE_KEYCHAIN_SERVICE_PREFIX;

use super::support::{markup_between, strip_rust_comments};

/// The value seat's table, as the product reads it.
const TABLE: &str = include_str!("../../../zerocode-core/fixtures/type-value/models.json");
const MARKUP: &str = include_str!("../../../../ui/index.html");
const SETTINGS: &str = include_str!("../../../../ui/shell-settings.js");
const I18N: &str = include_str!("../../../../ui/shell-i18n.js");

/// Where the card's script begins; it ends where the next section does.
const SCRIPT_OPENS: &str = "/* ---- the key typing into a page's fields asks with";

fn table_rows() -> Vec<Value> {
    let table: Value = serde_json::from_str(TABLE).expect("the seat's table");
    table["rows"].as_array().expect("its rows").clone()
}

/// Every value of `fields` any row of the table holds.
fn words_of(fields: &[&str]) -> Vec<String> {
    let mut words: Vec<String> = table_rows()
        .iter()
        .flat_map(|row| {
            fields
                .iter()
                .filter_map(|field| row[*field].as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .collect();
    words.sort();
    words.dedup();
    words
}

/// The card's markup: the card, and the slot it draws each key from.
fn card_markup() -> &'static str {
    let from = MARKUP
        .find("id=\"type-value-card\"")
        .expect("the Computer Use pane has the key card");
    markup_between(&MARKUP[from..], "id=\"type-value-card\"", "</template>")
}

/// The card's script, up to the next section.
fn card_script() -> &'static str {
    let from = SETTINGS
        .find(SCRIPT_OPENS)
        .expect("the settings script has the key card's section");
    let rest = &SETTINGS[from..];
    let to = rest[SCRIPT_OPENS.len()..]
        .find("\n/* ---- ")
        .map_or(rest.len(), |at| at + SCRIPT_OPENS.len());
    &rest[..to]
}

/// No key name, row, model, endpoint or keychain item of the table is written
/// anywhere in the window's markup, settings script or catalogs, and the
/// card's own markup and script name no road, no client and no key-store
/// prefix either: every word a slot shows is the backend's answer, so a row
/// the table gains, loses or chooses reaches the card with no edit here.
#[test]
fn the_key_card_spells_nothing_of_the_seats_table() {
    let anywhere = words_of(&["id", "model", "credentialKey", "keychainService", "baseUrl"]);
    assert!(!anywhere.is_empty(), "the table read as empty");
    for (file, source) in [
        ("index.html", MARKUP),
        ("shell-settings.js", SETTINGS),
        ("shell-i18n.js", I18N),
    ] {
        for word in &anywhere {
            assert!(!source.contains(word.as_str()), "{file} spells `{word}`");
        }
    }
    let mut in_the_card = words_of(&["road", "clientFingerprint", "host"]);
    in_the_card.push(SERVICE_KEYCHAIN_SERVICE_PREFIX.to_string());
    for (part, source) in [("markup", card_markup()), ("script", card_script())] {
        for word in &in_the_card {
            assert!(
                !source.contains(word.as_str()),
                "the card's {part} spells `{word}`"
            );
        }
    }
}

/// The card lists, saves and removes through three commands the window
/// registers, and its script asks each of them.
#[test]
fn the_key_card_asks_registered_commands() {
    let main = include_str!("../../src/main.rs");
    let handlers = main
        .split_once("tauri::generate_handler![")
        .map(|(_, list)| list)
        .expect("the handler list");
    for command in [
        "type_value_keys",
        "save_type_value_key",
        "remove_type_value_key",
    ] {
        assert!(
            handlers.contains(&format!("{command},")),
            "{command} is not registered"
        );
        assert!(
            card_script().contains(&format!("invoke(\"{command}\"")),
            "the card never asks {command}"
        );
    }
}

/// The keychain item a slot keeps its key in is the one the walk's writer
/// reads, named by the writer's own function (`value::key_service`), and a
/// slot is offered only for a road the writer takes (`value::endpoint_of`):
/// the backend spells no key-store prefix of its own. It keeps, reads and
/// forgets a key through the TypeSafe card's own doors, which that card goes
/// through too — one trim, one empty-key refusal, one reading of "saved".
#[test]
fn the_key_card_keeps_the_key_where_the_walk_reads_it() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let read = |name: &str| {
        let source =
            std::fs::read_to_string(src.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"));
        let shipped = source
            .split_once("\n#[cfg(test)]\nmod tests {")
            .map_or(source.as_str(), |(code, _)| code);
        strip_rust_comments(shipped)
    };
    let card = read("type_value_keys.rs");
    for needle in [
        "key_service(",
        "endpoint_of(",
        "key_saved_at(",
        "save_key_at(",
        "remove_key_at(",
    ] {
        assert!(card.contains(needle), "type_value_keys.rs lost `{needle}`");
    }
    for spelled in [
        "SERVICE_KEYCHAIN_SERVICE_PREFIX",
        SERVICE_KEYCHAIN_SERVICE_PREFIX,
        ".write(",
        ".delete(",
        ".trim()",
    ] {
        assert!(
            !card.contains(spelled),
            "type_value_keys.rs spells `{spelled}` itself"
        );
    }
    let typesafe = read("typesafe_settings.rs");
    for door in ["save_key_at(", "remove_key_at(", "key_saved_at("] {
        assert!(
            typesafe.matches(door).count() >= 2,
            "the TypeSafe card does not go through `{door}` itself"
        );
    }
}

/// The settings harness keeps a slot's key where the product does: its
/// key-store prefix is the harness crate's.
#[test]
fn the_settings_harness_mirrors_the_key_stores_prefix() {
    let harness = include_str!("../../../../ui/tests/settings.mjs");
    assert!(
        harness.contains(&format!(
            "const TYPE_VALUE_KEY_PREFIX = \"{SERVICE_KEYCHAIN_SERVICE_PREFIX}\";"
        )),
        "the harness's key-store prefix is not {SERVICE_KEYCHAIN_SERVICE_PREFIX}"
    );
}
