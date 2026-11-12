//! zo's desk guard switch (v1.1.146): what a test of one function cannot see
//! from inside it. The behaviour is the module's own tests
//! (`src/cmd/desk_guard.rs`) and the settings suite (`ui/tests/settings.mjs`).
//! This contract holds that the write that takes the guard down
//! (`set_desk_guard`) is reached by the person's own hand and by nothing
//! else: not by a press an agent makes in the window, not by a CLI verb, an
//! agent tool or a ledger verb, and not by a page a work browser shows.
//!
//! One road it does not hold, and no person-only switch does: a desktop
//! press an agent sends with `--allow-self` once the person has allowed the
//! shared mouse and keyboard lands on this window as a trusted event, which
//! the page reads as the person's hand (review t-105852, A).
//!
//! It holds the window's roads. The file the switch writes is the person's
//! own, and a model allowed to write anywhere can still edit it — zo's guard
//! is a railing, not a boundary (`zo-ide/crates/runtime/src/desk_guard.rs`).

use std::path::{Path, PathBuf};

use super::support::{block_after, strip_comments};

/// The one module that reads and writes the key.
const MODULE: &str = "zerocode-shell/src/cmd/desk_guard.rs";
/// This contract, which quotes what it holds.
const CONTRACT: &str = "zerocode-shell/tests/source_contracts/desk_guard.rs";

/// Every Rust file below `crates/` that may name the module or its commands,
/// with the number of lines that do. A file that is not here and names them
/// is a new road to the switch: it fails this contract until someone decides
/// whether an agent can walk it.
const NAMES_THE_COMMANDS: &[(&str, usize)] = &[
    // `pub(crate) mod desk_guard;` and the re-export of the two commands.
    ("zerocode-shell/src/cmd/mod.rs", 2),
    // The import, and the two commands in the window's one handler list.
    ("zerocode-shell/src/main.rs", 3),
    // The write, on the person-only list.
    ("zerocode-shell/src/computer_use/self_surface.rs", 1),
    // `mod desk_guard;` — this contract — and the module among the backend
    // files that keep their tests beside them.
    ("zerocode-shell/tests/source_contracts/main.rs", 2),
    // The module among the backend sources the other contracts read.
    ("zerocode-shell/tests/source_contracts/support.rs", 2),
];

fn repository() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// A file of the repository, read when the test runs: a missing one fails
/// this contract alone, not the whole target it sits in.
fn file(path: &str) -> String {
    std::fs::read_to_string(repository().join(path)).unwrap_or_else(|why| panic!("{path}: {why}"))
}

/// Every `.rs` below `crates/`, by its path from there. No hand list: a
/// hand list misses the file that was added, which is the failure this
/// contract is here for.
fn crate_sources() -> Vec<(String, String)> {
    let crates = repository().join("crates");
    let mut found = Vec::new();
    let mut pending = vec![crates.clone()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("a crates directory") {
            let path = entry.expect("a crates entry").path();
            let name = path.file_name().and_then(|name| name.to_str());
            if path.is_dir() {
                if !matches!(name, Some("target" | "node_modules" | ".git")) {
                    pending.push(path);
                }
            } else if path.extension().and_then(|it| it.to_str()) == Some("rs") {
                let name = path
                    .strip_prefix(&crates)
                    .expect("a source stays below crates")
                    .to_string_lossy()
                    .replace('\\', "/");
                found.push((
                    name,
                    std::fs::read_to_string(&path).expect("a source reads"),
                ));
            }
        }
    }
    found.sort();
    assert!(
        found.len() > 300,
        "the walk below crates/ found only {} files",
        found.len()
    );
    found
}

/// The window's scripts, comments off: `ui/*.js`, by file name.
fn window_scripts() -> Vec<(String, String)> {
    let ui = repository().join("ui");
    let mut found = Vec::new();
    for entry in std::fs::read_dir(&ui).expect("ui reads") {
        let path = entry.expect("a ui entry").path();
        if path.extension().and_then(|it| it.to_str()) != Some("js") {
            continue;
        }
        let name = path
            .file_name()
            .expect("a script has a name")
            .to_string_lossy()
            .into_owned();
        let source = std::fs::read_to_string(&path).expect("a script reads");
        found.push((name, strip_comments(&source)));
    }
    found.sort();
    assert!(found.len() > 20, "ui/ holds only {} scripts", found.len());
    found
}

#[test]
fn the_switch_that_takes_the_desk_guard_down_is_reached_by_the_persons_hand_alone() {
    // ── 1. A press an agent makes in the window ─────────────────────────
    // The second wall: the write is on the person-only list, and the
    // window's one invoke handler asks that list for every command — so a
    // call that does not say the person's hand made it is refused by name
    // (the hands themselves: `cmd::desk_guard::tests::
    // only_the_persons_hand_turns_the_guard`).
    let road = file("crates/zerocode-shell/src/computer_use/self_surface.rs");
    let opens = "pub(crate) const PERSON_ONLY_COMMANDS: &[&str] = &[";
    let start = road
        .find(opens)
        .expect("the road lists its person-only commands")
        + opens.len();
    let person_only = &road[start..start + road[start..].find("];").expect("the list closes")];
    assert!(
        person_only.contains("\"set_desk_guard\","),
        "the write that turns zo's desk guard is not on the person-only list"
    );
    assert!(
        !person_only.contains("\"desk_guard_setting\""),
        "the read is not the person's alone: an agent looking at the settings may see it"
    );
    let main = file("crates/zerocode-shell/src/main.rs");
    assert!(
        main.contains(".invoke_handler(person_only_gate(tauri::generate_handler!["),
        "the window's commands are no longer registered through the second wall"
    );

    // The first wall: the row carries the person's mark, which the page's
    // own press road reads before it presses anything.
    let markup = file("ui/index.html");
    let control = markup
        .find("id=\"computer-desk-guard\"")
        .expect("the Computer Use settings have the switch");
    let label = &markup[markup[..control]
        .rfind("<label")
        .expect("the switch has a label")..control];
    assert!(
        label.contains("data-person-only") && !label.contains("</label>"),
        "the switch's own label does not carry the person's mark: {label}"
    );
    assert!(
        file("ui/shell-self-act.js")
            .contains("const SELF_ACT_PERSON_ONLY = \"[data-person-only]\";"),
        "the page's press road no longer reads the person's mark"
    );

    // The page asks for the write in one place, with the hand of the press
    // that flipped the switch — read before the handler first waits, so it
    // is that event's hand and not a later one's.
    let scripts = window_scripts();
    for (name, script) in &scripts {
        assert!(
            name == "shell-settings.js" || !script.contains("set_desk_guard"),
            "{name} names the write: a second place the page can ask for it"
        );
    }
    let settings = &scripts
        .iter()
        .find(|(name, _)| name == "shell-settings.js")
        .expect("the settings script")
        .1;
    assert_eq!(settings.matches("set_desk_guard").count(), 1);
    assert_eq!(
        settings
            .matches("invoke(\"set_desk_guard\", { on }, { hand })")
            .count(),
        1,
        "the write carries the answer and the hand of the press, nothing else"
    );
    let handler = block_after(
        settings,
        "el(\"computer-desk-guard\").addEventListener(\"change\", async () => {",
    );
    let hand = handler
        .find("const hand = handNow();")
        .expect("the handler reads the hand of the press");
    let waits = handler
        .find("await ")
        .expect("the handler waits for the write");
    assert!(
        hand < waits,
        "the hand is read after the handler waits: it would be another event's"
    );

    // ── 2. A CLI verb, an agent tool, a ledger verb ─────────────────────
    // Every road into the window from outside its page ends in Rust below
    // `crates/`. Nothing there names the key but the one module, and nothing
    // names the module or its commands but the places a command is
    // registered — so no verb, tool or event handler can reach the writer.
    let mut naming: Vec<(String, usize)> = Vec::new();
    let mut writers: Vec<String> = Vec::new();
    for (name, source) in crate_sources() {
        if name == CONTRACT {
            continue;
        }
        if source.contains("update_zo_settings_root") {
            writers.push(name.clone());
        }
        if name == MODULE {
            continue;
        }
        assert!(
            !source.contains("deskGuard") && !source.contains("DESK_GUARD"),
            "{name} names zo's desk guard key: only {MODULE} reads or writes it"
        );
        let lines = source
            .lines()
            .filter(|line| line.contains("desk_guard"))
            .count();
        if lines > 0 {
            naming.push((name, lines));
        }
    }
    let mut allowed: Vec<(String, usize)> = NAMES_THE_COMMANDS
        .iter()
        .map(|(name, lines)| ((*name).to_string(), *lines))
        .collect();
    allowed.sort();
    assert_eq!(
        naming, allowed,
        "a file below crates/ names the desk guard's module or commands that this contract \
         does not know: a new road to the switch is decided here first"
    );
    // A writer that named no key of its own — one handed its key by its
    // caller — would pass the walk above. The files that write zo's settings
    // are these: the writer itself, the judgment provider's and the model
    // connections' (each writes its own fixed keys — the provider's, the
    // `smart` object's, the summon profiles', `model`), and the module. A
    // new one is read for the key it writes before it joins this list.
    assert_eq!(
        writers,
        [
            "zerocode-shell/src/api_routers.rs",
            MODULE,
            "zerocode-shell/src/model_connections.rs",
            "zerocode-shell/src/typesafe_settings.rs",
        ],
        "a file writes zo's settings that this contract has not read"
    );
    let module = file(&format!("crates/{MODULE}"));
    assert_eq!(
        module.matches("update_zo_settings_root").count(),
        1,
        "the module writes zo's settings in one place"
    );
    assert!(
        block_after(&module, "fn turn_in(").contains("update_zo_settings_root_if_changed("),
        "the one write is the person's command's"
    );
    assert_eq!(
        module.matches("turn_in(").count() - module.matches("fn turn_in(").count(),
        1 + module
            .split("#[cfg(test)]")
            .nth(1)
            .expect("the module's tests")
            .matches("turn_in(")
            .count(),
        "the writer is called by the one command, and by the module's tests"
    );
    // zo's own tool for changing settings cannot name the key either: the
    // model has no setting to ask for.
    assert!(
        !file("zo-ide/crates/tools/src/misc_tools/config_tools.rs").contains("deskGuard"),
        "zo's Config tool knows the desk guard's key"
    );

    // ── 3. A page a work browser shows ──────────────────────────────────
    // A browser pane is a webview of the same window showing somebody
    // else's page. Two devices keep it from the window's commands, and the
    // list of webviews in the capability file is neither: Tauri asks the
    // ACL about an app command only for a remote origin (this app declares
    // no command manifest), so what refuses a guest page is that no
    // capability grants a remote origin — and what keeps a pane off the
    // app's own origin, where the window's script would boot with IPC, is
    // the one function both the address bar and a page's own navigation ask.
    let browser = file("crates/zerocode-shell/src/browser_runtime.rs");
    let browsable = block_after(&browser, "pub(super) fn browsable_target(");
    assert!(
        browsable.contains("host == \"tauri.localhost\" || host.ends_with(\".tauri.localhost\")")
            && browsable.contains("_ => false,"),
        "a browser pane may stand on the app's own origin:\n{browsable}"
    );
    assert!(
        !browsable.contains("\"tauri\""),
        "a browser pane is let onto the app's own scheme:\n{browsable}"
    );
    let capabilities = repository().join("crates/zerocode-shell/capabilities");
    let granted: Vec<String> = std::fs::read_dir(&capabilities)
        .expect("the capabilities folder")
        .map(|entry| {
            entry
                .expect("a capability")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .filter(|name| name.ends_with(".json"))
        .collect();
    assert_eq!(
        granted,
        ["default.json"],
        "a second capability file may grant what this one withholds: read it here first"
    );
    let capability = file("crates/zerocode-shell/capabilities/default.json");
    assert!(
        !capability.contains("\"remote\":"),
        "the capability grants a remote origin: a guest page could invoke the window's commands"
    );
    // Not what stops a guest page, but what the file says of itself: its
    // plugin permissions go to the window's own two documents by name.
    assert!(
        capability.contains("\"webviews\": [\"main\", \"board-popout\"]")
            && !capability.contains("\"windows\":"),
        "the capability no longer names the window's own two documents"
    );
}
