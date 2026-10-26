//! The plain-text secrets line (t-76698): what a test of one function cannot
//! see from inside it. The behaviour is the module's own tests
//! (`settings_secrets_tests.rs`, against a fake zo) and the window suite
//! (`ui/tests/settings-secrets.mjs`). These contracts hold that a value has
//! nowhere to go: the page asks for names and sends nothing, the road that
//! holds a value names no log, print or record, and the move is the person's.

use std::path::Path;

use super::support::{block_after, block_after_css, strip_rust_comments};

const MODULE: &str = include_str!("../../src/settings_secrets.rs");
const COMMANDS: &str = include_str!("../../src/cmd/settings_secrets.rs");
const SELF_SURFACE: &str = include_str!("../../src/computer_use/self_surface.rs");
const KEY_SAVES: &str = include_str!("../../src/typesafe_settings.rs");

/// A window file, read when the test runs: a missing one fails this contract
/// alone, not the whole target it sits in.
fn ui_file(name: &str) -> String {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    std::fs::read_to_string(ui.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// The places a page keeps things. The line names none of them.
const PAGE_KEEPS: &[&str] = &[
    "localStorage",
    "sessionStorage",
    "indexedDB",
    "console.",
    "tellWindowLog",
    "document.cookie",
    "history.",
    "fetch(",
    "XMLHttpRequest",
    "navigator.clipboard",
];

/// The places a backend keeps things, or prints them.
const BACKEND_KEEPS: &[&str] = &[
    "eprintln!",
    "println!",
    "tracing::",
    "log::",
    "record(",
    "ledger",
    "crumbs::",
    "{:?}",
];

#[test]
fn the_page_asks_for_names_and_sends_nothing_back() {
    let page = ui_file("shell-settings-secrets.js");
    for keep in PAGE_KEEPS {
        assert!(
            !page.contains(keep),
            "the line names {keep}, a place the page keeps things"
        );
    }
    assert_eq!(page.matches("invoke(").count(), 2, "one listing, one move");
    assert_eq!(
        page.matches("invoke(\"settings_secret_literals\")").count(),
        1
    );
    assert_eq!(
        page.matches("invoke(\"move_settings_secrets\", {}, { hand })")
            .count(),
        1,
        "the move carries no value, path or name from the page — only the person's hand"
    );
    let markup = ui_file("index.html");
    assert!(
        markup.contains("./shell-settings-secrets.js"),
        "the page loads the line's script"
    );
    let at = markup
        .find("id=\"settings-secrets\"")
        .expect("the settings frame has the line");
    let tag = &markup[at..at + markup[at..].find('>').expect("the tag closes")];
    assert!(tag.contains(" hidden"), "the line starts hidden: {tag}");
}

#[test]
fn the_road_that_holds_a_value_keeps_it_nowhere() {
    let module = strip_rust_comments(MODULE);
    for opens in [
        "fn move_one(",
        "fn cell(",
        "fn read_pinned(",
        "fn successor(",
        "fn ask(",
        "fn said_without(",
    ] {
        let road = block_after(&module, opens);
        for keep in BACKEND_KEEPS {
            assert!(
                !road.contains(keep),
                "{opens} names {keep}, a place a value could be kept"
            );
        }
    }
    let move_one = block_after(&module, "fn move_one(");
    assert!(
        move_one.contains("typesafe_settings::save_key_unless_another_at("),
        "the value is kept through the window's one keychain save, not a road of its own"
    );
    let saves = strip_rust_comments(KEY_SAVES);
    for opens in [
        "pub fn save_key_at(",
        "pub(crate) fn save_key_unless_another_at<",
        "pub fn remove_key_at(",
    ] {
        let save = block_after(&saves, opens);
        assert!(
            save.contains("KeyItems::take()"),
            "{opens} takes the one guard over keychain items"
        );
    }
    for opens in [
        "pub fn save_key_at(",
        "pub(crate) fn save_key_unless_another_at<",
    ] {
        assert!(
            block_after(&saves, opens).contains("save_key_held("),
            "{opens} writes through the one save"
        );
    }
    let args_at = move_one.find("let args = [").expect("the move's arguments");
    let args = &move_one[args_at..args_at + move_one[args_at..].find("];").expect("closed")];
    let asks = move_one
        .lines()
        .filter(|line| line.contains("zo.ask(&["))
        .collect::<Vec<_>>();
    assert!(!asks.is_empty(), "the check is asked in place");
    for said in asks.iter().copied().chain(std::iter::once(args)) {
        assert!(
            !said.contains("value"),
            "zo is never handed the value: {said}"
        );
    }
    let commands = strip_rust_comments(COMMANDS);
    for keep in BACKEND_KEEPS {
        assert!(!commands.contains(keep), "the commands name {keep}");
    }
}

#[test]
fn only_the_persons_hand_moves_a_secret() {
    let list = |opens: &str| {
        let at = SELF_SURFACE.find(opens).expect("the list is defined");
        &SELF_SURFACE[at..at + SELF_SURFACE[at..].find("\n];").expect("the list closes")]
    };
    let gate = list("pub(crate) const PERSON_ONLY_COMMANDS");
    let reads = list("pub(crate) const AGENT_ALLOWED_SENSITIVE");
    assert!(
        reads.contains("\"settings_secret_literals\""),
        "the listing is a read an agent may make: names, never a value"
    );
    assert!(
        gate.contains("\"move_settings_secrets\""),
        "the move is a credential written, the person's alone"
    );
    assert!(
        !gate.contains("\"settings_secret_literals\""),
        "the listing reads names only and rides the settings' own drawing"
    );
}

#[test]
fn the_line_is_drawn_with_tokens_only() {
    let css = ui_file("shell.css");
    for opens in [".settings-secrets-names {", ".settings-secrets-actions {"] {
        let rule = block_after_css(&css, opens);
        for literal in ["#", "rgb(", "hsl("] {
            assert!(!rule.contains(literal), "{opens} spells a colour: {rule}");
        }
    }
}
