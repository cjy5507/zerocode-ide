//! The secret card (herdr 3, t-26596): what a test of one function cannot see
//! from inside it. The behaviour is the answer door's own tests (`answer_door`),
//! core's rule (`zerocode_core::secret_prompt`) and the window suite
//! (`ui/tests/secret-prompt.mjs`). These contracts hold where a typed value may
//! go — into the pane, once, through the answer door — and that nothing else
//! keeps a copy of it: no storage, no console, no log line, no ledger record,
//! and no backend call but the one that types.

use std::path::Path;

use super::support::{block_after, strip_rust_comments};

const TERMINAL: &str = include_str!("../../src/cmd/terminal.rs");
const ANSWER_DOOR: &str = include_str!("../../src/answer_door.rs");
const MAIN: &str = include_str!("../../src/main.rs");
const WATCH: &str = include_str!("../../src/secret_watch.rs");

/// A window file, read when the test runs: a missing one fails this contract
/// alone, not the whole target it sits in.
fn ui_file(name: &str) -> String {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    std::fs::read_to_string(ui.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// The places a page keeps things. The card names none of them.
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

/// The places a backend keeps things, or prints them. The typing command and
/// the door it goes through name none of them.
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

/// The body of a struct with the given name, up to its closing brace.
fn struct_body<'a>(source: &'a str, name: &str) -> &'a str {
    let start = source
        .find(&format!("struct {name} {{"))
        .unwrap_or_else(|| panic!("struct {name} is not defined"));
    let rest = &source[start..];
    &rest[..rest.find("\n}").map_or(rest.len(), |end| end + 2)]
}

#[test]
fn the_card_keeps_no_copy_of_the_value_and_sends_it_in_one_call() {
    let card = ui_file("shell-secret.js");
    assert!(
        card.contains("registerAskKind(\"secret\""),
        "the card is the popup's secret kind"
    );
    for keep in PAGE_KEEPS {
        assert!(
            !card.contains(keep),
            "the card names {keep}, a place the page keeps things"
        );
    }
    assert_eq!(
        card.matches("invoke(\"answer_secret\"").count(),
        1,
        "exactly one call types the value"
    );
    assert_eq!(
        card.matches("invoke(").count(),
        1,
        "the card calls the backend only to answer; the backend says when a pane asks"
    );
}

#[test]
fn the_field_is_a_password_input_that_no_form_owns() {
    let markup = ui_file("index.html");
    let at = markup
        .find("id=\"ask-secret-input\"")
        .expect("the popup has the secret field");
    let tag_start = markup[..at].rfind('<').expect("the field is a tag");
    let tag = &markup[tag_start..at + markup[at..].find('>').expect("the tag closes")];
    for attribute in [
        "type=\"password\"",
        "autocomplete=\"new-password\"",
        "autocapitalize=\"none\"",
        "autocorrect=\"off\"",
        "spellcheck=\"false\"",
    ] {
        assert!(
            tag.contains(attribute),
            "the field carries {attribute}: {tag}"
        );
    }
    let before = &markup[..at];
    let inside_a_form = before
        .rfind("<form")
        .is_some_and(|open| before.rfind("</form>").is_none_or(|close| close < open));
    assert!(
        !inside_a_form,
        "no form owns the field, so no browser keeps what is sent from it"
    );
    assert!(
        markup.contains("./shell-secret.js"),
        "the page loads the card's script"
    );
}

#[test]
fn the_value_is_typed_through_the_answer_door_and_recorded_nowhere() {
    let terminal = strip_rust_comments(TERMINAL);
    let command = block_after(&terminal, "fn answer_secret(");
    assert!(
        command.contains("answer_secret_into"),
        "the command hands the value to the answer road, which types through the answer door"
    );
    for keep in BACKEND_KEEPS {
        assert!(
            !command.contains(keep),
            "answer_secret names {keep}, a place the value could be kept"
        );
    }
    let answer_door = strip_rust_comments(ANSWER_DOOR);
    let road = block_after(&answer_door, "fn answer_secret_into(");
    assert!(
        road.contains("type_secret_if_up") && road.contains("value_is_typable"),
        "the road checks the value and then types it through the door"
    );
    for keep in BACKEND_KEEPS {
        assert!(!road.contains(keep), "answer_secret_into names {keep}");
    }
    let door = block_after(&answer_door, "fn type_secret_if_up(");
    assert!(
        door.contains("secret_route_open(&pty"),
        "the door asks the route before it writes, under the same lock"
    );
    assert!(
        door.contains("write_input(&typed)"),
        "the door writes the value and its return to the pane, and nowhere else"
    );
    assert!(
        door.contains("Zeroizing::new(Vec::with_capacity"),
        "the buffer that holds the value is wiped when it goes out of scope"
    );
    for keep in BACKEND_KEEPS {
        assert!(!door.contains(keep), "type_secret_if_up names {keep}");
    }
}

#[test]
fn the_question_is_heard_without_a_value_and_the_card_listens_for_it() {
    let watch = strip_rust_comments(WATCH);
    assert!(
        watch.contains("\"term:secret\""),
        "the watcher says a pane's question on the term:secret event"
    );
    let question = struct_body(&watch, "SecretQuestion");
    for field in ["kind:", "line:", "since:"] {
        assert!(question.contains(field), "SecretQuestion names {field}");
    }
    assert!(
        !question.contains("value"),
        "a pane's question is said without any value"
    );
    for keep in BACKEND_KEEPS {
        assert!(
            !watch.contains(keep),
            "secret_watch names {keep}, a place the value could be kept"
        );
    }
    let card = ui_file("shell-secret.js");
    assert!(
        card.contains("listen(\"term:secret\""),
        "the card hears the watcher's event instead of asking the backend"
    );
    assert!(
        MAIN.contains("answer_secret"),
        "answer_secret is on the window's command list"
    );
}

#[test]
fn every_secret_word_is_translated_in_the_four_catalogs_beside_korean() {
    let catalog = ui_file("shell-i18n.js");
    for key in [
        "secret.title.password",
        "secret.title.passphrase",
        "secret.title.pin",
        "secret.send",
        "secret.label",
        "secret.hint",
        "secret.empty",
        "secret.busy",
        "secret.invalid",
    ] {
        assert_eq!(
            catalog.matches(&format!("\"{key}\":")).count(),
            4,
            "{key} is translated in en, ja, zh and es (Korean is the source)"
        );
    }
}
