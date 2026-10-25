//! ZeroCode's own window as a Computer Use target (t-73906): what a test of
//! one function cannot see from inside it. The behaviour is the road's own
//! tests (`computer_use::self_surface`), core's rule (`self_surface::judge`)
//! and the window suite (`ui/tests/self-act.mjs`); these contracts hold that
//! every person-only command checks the second wall, that the controls the
//! window keeps for the person carry its mark, and that the page presses its
//! controls without an OS-like event, a focus move or the clipboard.

use std::path::Path;

use super::quiet_children::{code_only, shell_sources};
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

/// Every person-only command is a window command, defined once, and asks the
/// second wall before it does anything.
#[test]
fn every_person_only_command_asks_the_second_wall_first() {
    let sources = shell_sources();
    for name in person_only_commands() {
        let opens = format!("fn {name}(");
        let defined: Vec<&(String, String)> = sources
            .iter()
            .filter(|(file, source)| !file.contains("test") && code_only(source).contains(&opens))
            .collect();
        assert_eq!(
            defined.len(),
            1,
            "{name} is defined once: {:?}",
            defined.iter().map(|(file, _)| file).collect::<Vec<_>>()
        );
        let (file, source) = defined[0];
        let body = block_after(source, &opens);
        assert!(
            body.contains(&format!("self_surface::person_only(\"{name}\")")),
            "{file}: {name} does not ask `self_surface::person_only(\"{name}\")`"
        );
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
/// resume, a board card's answers, a terminal tab's close.
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
