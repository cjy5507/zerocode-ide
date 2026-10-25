//! ZeroCode's own window as a Computer Use target (t-73906): what a test of
//! one function cannot see from inside it. The behaviour is the road's own
//! tests (`computer_use::self_surface`), core's rule (`self_surface::judge`)
//! and the window suite (`ui/tests/self-act.mjs`); these contracts hold that
//! every person-only command checks the second wall, that the controls the
//! window keeps for the person carry its mark, and that the page presses its
//! controls without an OS-like event, a focus move or the clipboard.

use std::path::Path;

use super::quiet_children::code_only;
use super::support::block_after;

const ROAD: &str = include_str!("../../src/computer_use/self_surface.rs");

fn ui_file(name: &str) -> String {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    std::fs::read_to_string(ui.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// The person-only commands, read off the road's own list.
fn person_only_commands() -> Vec<String> {
    let opens = "pub(crate) const PERSON_ONLY_COMMANDS: &[&str] = &[";
    let start = ROAD
        .find(opens)
        .expect("the road lists its person-only commands")
        + opens.len();
    let list = &ROAD[start..start + ROAD[start..].find("];").expect("the list closes")];
    let names: Vec<String> = list
        .split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect();
    assert!(names.len() > 10, "the list reads: {names:?}");
    names
}

/// A list of the road's, read off its source: the quoted names up to `];`.
fn road_list(opens: &str) -> Vec<String> {
    let start = ROAD
        .find(opens)
        .unwrap_or_else(|| panic!("the road has {opens}"))
        + opens.len();
    let list = &ROAD[start..start + ROAD[start..].find("];").expect("the list closes")];
    list.split('"')
        .skip(1)
        .step_by(2)
        .map(str::to_string)
        .collect()
}

/// The commands the window registers, by the name the page invokes them by.
fn registered_commands() -> Vec<String> {
    let main = code_only(include_str!("../../src/main.rs"));
    let opens = "tauri::generate_handler![";
    let start = main.find(opens).expect("the window registers its commands") + opens.len();
    let list = &main[start..start + main[start..].find(']').expect("the list closes")];
    list.split(',')
        .map(|entry| {
            entry
                .trim()
                .rsplit("::")
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .filter(|name| !name.is_empty())
        .collect()
}

/// Every command passes the second wall in one place: the window's invoke
/// handler is wrapped, so a command on the person-only list refuses while an
/// agent presses in the window whichever crate defines it.
#[test]
fn the_invoke_handler_asks_the_second_wall_for_every_command() {
    let main = code_only(include_str!("../../src/main.rs"));
    assert!(
        main.contains(".invoke_handler(person_only_gate(tauri::generate_handler!["),
        "the window's commands are registered through person_only_gate"
    );
    let registered = registered_commands();
    for name in person_only_commands() {
        assert!(
            registered.contains(&name),
            "{name} is on the person-only list but not registered"
        );
    }
}

/// The rule, not a hand list: a registered command whose name touches a key,
/// a secret, a token, a password, a credential, an account or a login is on
/// the person-only list, or on the list of such names an agent may run (a
/// read, not a write), each with its reason in the road's source. A new
/// command of that kind fails here until someone decides which it is.
#[test]
fn a_command_named_for_a_credential_or_an_account_is_decided_for() {
    const WORDS: [&str; 7] = [
        "key",
        "secret",
        "token",
        "password",
        "credential",
        "account",
        "login",
    ];
    let person = person_only_commands();
    let allowed = road_list("pub(crate) const AGENT_ALLOWED_SENSITIVE: &[&str] = &[");
    let registered = registered_commands();
    let undecided: Vec<&String> = registered
        .iter()
        .filter(|name| WORDS.iter().any(|word| name.contains(word)))
        .filter(|name| !person.contains(name) && !allowed.contains(name))
        .collect();
    assert!(
        undecided.is_empty(),
        "undecided credential or account commands: {undecided:?}"
    );
    for name in &allowed {
        assert!(
            registered.contains(name),
            "{name} is allowed but not registered"
        );
        assert!(!person.contains(name), "{name} is on both lists");
    }
}

/// The page presses with the document's own calls: no synthetic key or
/// pointer event, no focus move, no clipboard.
#[test]
fn the_page_presses_without_keys_focus_or_the_clipboard() {
    let page = ui_file("shell-self-act.js");
    for forbidden in [
        ".focus(",
        "KeyboardEvent",
        "PointerEvent",
        "MouseEvent",
        "clipboard",
        "execCommand",
    ] {
        assert!(
            !page.contains(forbidden),
            "shell-self-act.js uses {forbidden}"
        );
    }
    assert!(page.contains("listen(\"computer:self-act\""));
    assert!(page.contains("invoke(\"computer_self_answer\""));
    let index = ui_file("index.html");
    assert!(
        index.contains("<script defer src=\"./shell-self-act.js\"></script>"),
        "index.html loads the page's half"
    );
}

/// The controls the window keeps for the person carry the mark the page
/// reads (`data-person-only`): the coordinator's seat card, the stop's
/// resume, a board card's answers, the inbox's answer, a terminal tab's close.
#[test]
fn the_persons_controls_carry_the_person_only_mark() {
    let index = ui_file("index.html");
    for id in [
        "coordinator-scrim",
        "computer-band-resume",
        "update-notice",
        "update-card",
    ] {
        let at = index
            .find(&format!("id=\"{id}\""))
            .unwrap_or_else(|| panic!("index.html has #{id}"));
        let tag_start = index[..at].rfind('<').unwrap_or(at);
        let tag_end = at + index[at..].find('>').unwrap_or(0);
        assert!(
            index[tag_start..tag_end].contains("data-person-only"),
            "#{id} carries data-person-only"
        );
    }
    let desk = index
        .find("class=\"task-board-desk\"")
        .expect("index.html has the coordinator desk");
    let desk_tag = &index[desk..desk + index[desk..].find('>').unwrap_or(0)];
    assert!(
        desk_tag.contains("data-person-only"),
        "the coordinator desk carries data-person-only"
    );
    // The inbox answers through the desk's own roads (t-67483), so its answer
    // and its buttons carry the desk's mark.
    let inbox = ui_file("shell-inbox.js");
    let inbox = block_after(&inbox, "function buildInboxView(");
    for part in [
        "reply.dataset.personOnly = \"\"",
        "actions.dataset.personOnly = \"\"",
        // A row opens its letter and a read leaves the ledger's ack (t-74825).
        "list.dataset.personOnly = \"\"",
    ] {
        assert!(inbox.contains(part), "the inbox's answer lost `{part}`");
    }
    let shell = ui_file("shell.js");
    for panel in ["function askPanelNode(", "function approvalPanelNode("] {
        assert!(
            block_after(&shell, panel).contains("panel.dataset.personOnly = \"\""),
            "{panel} marks its panel person-only"
        );
    }
    let status = ui_file("shell-status.js");
    assert!(
        block_after(&status, "function buildTabNode(").contains("dataset.personOnly"),
        "a terminal tab's close is marked person-only"
    );
}

/// The share holds every provider step: the one road to the helper and to
/// the page asks it before anything else, and a command is admitted to it
/// before it acts. Taking control and handing back are the person's.
#[test]
fn every_step_asks_the_share_and_the_band_is_the_persons() {
    let call = code_only(include_str!("../../src/computer_use/mod.rs"));
    let body = block_after(&call, "pub fn call(method: &str, params: Value)");
    let share = body
        .find("share::ensure_current()?")
        .expect("call asks the share");
    let route = body
        .find("self_surface::route(")
        .expect("call routes the page");
    assert!(
        share < route,
        "the share is asked before the page or the helper"
    );
    let runtime = code_only(include_str!("../../src/agent_tools_runtime.rs"));
    assert!(
        block_after(&runtime, "fn answer_computer_command_with(")
            .contains("computer_use::share::admit("),
        "a command is admitted to the share before it acts"
    );
    let person = person_only_commands();
    for name in [
        "computer_take_control",
        "computer_give_back",
        "computer_share_hand_over",
    ] {
        assert!(person.contains(&name.to_string()), "{name} is the person's");
    }
}

/// The ledger's ack of a letter is the person's: the desk and the inbox both
/// reach it, and the marks on their controls are the first wall (t-74825).
#[test]
fn the_ledger_ack_of_a_letter_is_a_person_only_command() {
    assert!(person_only_commands().contains(&"desk_ack".to_string()));
}

#[test]
fn requiring_a_share_is_a_persisted_person_only_setting() {
    let person = person_only_commands();
    assert!(person.contains(&"set_computer_share_required".to_string()));
    let settings = code_only(include_str!("../../src/cmd/settings.rs"));
    let body = block_after(&settings, "pub(crate) fn set_computer_share_required(");
    assert!(body.contains("commit_setting("));
    assert!(body.contains("setting_key::COMPUTER_SHARE_REQUIRED"));
    assert!(body.contains("settings.computer_share_required = on"));
}

#[test]
fn required_sharing_reads_the_saved_policy_and_rechecks_each_provider_step() {
    let runtime = code_only(include_str!("../../src/agent_tools_runtime.rs"));
    let install = block_after(&runtime, "fn install_share(app: AppHandle)");
    assert!(install.contains("share::install_required("));
    // R1 (t-74826): read at every provider step, so from memory — never the
    // disk and never the repository's lock a commit holds through its fsync
    // (t-67102). The person's toggle commits, and a commit is remembered.
    assert!(install.contains("remembered_settings("));
    assert!(
        !install.contains("load_settings("),
        "a step must not wait on the settings lock"
    );
    assert!(install.contains("snapshot.document.computer_share_required"));
    let road = code_only(include_str!("../../src/computer_use/share.rs"));
    let current = block_after(&road, "pub(crate) fn ensure_current()");
    assert!(current.contains("refresh_required()"));
    // A step in flight is judged without moving the share (t-74826).
    assert!(current.contains("held.share.still_holds(Reach"));
    assert!(!current.contains(".gate("));
}
