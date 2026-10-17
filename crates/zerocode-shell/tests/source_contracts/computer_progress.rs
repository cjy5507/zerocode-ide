//! The work screen panel and the person's stop (t-59372): what the window
//! suite (`ui/tests/computer-progress.mjs`) and the backend's own tests
//! (`computer_use::guard`, `computer_use::observe::owned_tests`,
//! `computer_use::marks`) cannot see from inside one function. A look's
//! picture leaves the window's process only for the window's own page, only
//! about a pane the window holds and only for the assignment that pane still
//! holds; the page keeps one picture and no copy of it; a stop is lifted on
//! both roads by the one resume; and no mark is pressed past a stop.

use std::path::Path;

use super::support::{block_after, strip_rust_comments};

const SETTINGS: &str = include_str!("../../src/cmd/settings.rs");
const RUNTIME: &str = include_str!("../../src/agent_tools_runtime.rs");
const MAIN: &str = include_str!("../../src/main.rs");
const OBSERVE: &str = include_str!("../../src/computer_use/observe.rs");
const MARKS: &str = include_str!("../../src/computer_use/marks.rs");

/// A window file, read when the test runs: a missing one fails this contract
/// alone, not the whole target it sits in.
fn ui_file(name: &str) -> String {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    std::fs::read_to_string(ui.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// The places a page keeps things, or keeps them coming. The panel names
/// none of them: its one picture goes when it closes.
const PAGE_KEEPS: &[&str] = &[
    "setInterval",
    "requestAnimationFrame",
    "localStorage",
    "sessionStorage",
    "indexedDB",
    "navigator.clipboard",
    "createObjectURL",
    "download",
];

#[test]
fn the_frame_door_answers_only_the_windows_own_page_about_a_pane_it_holds() {
    let settings = strip_rust_comments(SETTINGS);
    let door = block_after(&settings, "pub(crate) fn computer_progress_frame(");
    let checked = door
        .find("from_the_main_webview(&webview)?")
        .expect("the door checks which page asks");
    let held = door
        .find("held_pane(")
        .expect("the door asks whether the window holds the pane");
    let scoped = door
        .find("computer_frame_scope(")
        .expect("the door compares the assignment the pane holds now");
    let looked = door
        .find("owned_look(")
        .expect("the door reads the pane's own look");
    assert!(
        checked < held && held < scoped && scoped < looked,
        "who asks, the pane and its assignment are checked before a look is read: {door}"
    );
    for refusal in [
        "computer_progress_pane_unknown",
        "computer_progress_scope_unknown",
    ] {
        assert!(
            door.contains(refusal),
            "the door says {refusal} rather than answering nothing: {door}"
        );
    }
    assert!(
        strip_rust_comments(MAIN).contains("cmd::settings::computer_progress_frame,"),
        "the window registers the door"
    );
}

#[test]
fn a_look_is_told_to_the_windows_own_page_by_number_and_never_by_picture() {
    let runtime = strip_rust_comments(RUNTIME);
    assert_eq!(
        runtime.matches("\"computer:frame\"").count(),
        1,
        "one place tells the page of a look"
    );
    for (file, source) in [("cmd/settings.rs", SETTINGS), ("observe.rs", OBSERVE)] {
        assert!(
            !strip_rust_comments(source).contains("\"computer:frame\""),
            "{file} tells no page of a look"
        );
    }
    let notice = block_after(&runtime, "fn install_frame_notice(");
    assert!(
        notice.contains("emit_to(") && notice.contains("MAIN_WINDOW_LABEL"),
        "only the window's own page hears of a look: {notice}"
    );
    assert!(
        !notice.contains(".emit("),
        "no other webview hears of a look: {notice}"
    );
    assert!(
        notice.contains(r#"json!({ "term": term, "scope": scope, "seq": seq })"#),
        "the notice carries the pane, the assignment and the number, and nothing else: {notice}"
    );
    assert!(
        strip_rust_comments(OBSERVE).contains("type LookNotice = dyn Fn(&str, &str, u64)"),
        "the notice has no room for a picture"
    );
}

#[test]
fn the_panel_keeps_one_picture_no_copy_and_no_timer() {
    let page = ui_file("shell-computer.js");
    for keep in PAGE_KEEPS {
        assert!(
            !page.contains(keep),
            "shell-computer.js names {keep}, a place a page keeps things"
        );
    }
    assert_eq!(
        page.matches("invoke(\"computer_progress_frame\"").count(),
        1,
        "one call asks for a look"
    );
    assert_eq!(
        page.matches("listen(\"computer:frame\"").count(),
        1,
        "the page hears of looks in one place, once"
    );
    let markup = ui_file("index.html");
    let at = markup
        .find("id=\"computer-progress-frame\"")
        .expect("the panel has its picture");
    let tag_start = markup[..at].rfind('<').expect("the picture is a tag");
    let tag = &markup[tag_start..at + markup[at..].find('>').expect("the tag closes")];
    assert!(
        tag.starts_with("<img") && tag.contains("draggable=\"false\""),
        "the picture is an image that is not dragged out: {tag}"
    );
}

/// Who holds a work browser is the dedicated browser's own word (t-62464):
/// the panel reads the state shell-browser.js took in, in its revision order,
/// and leads to that browser's own bar. It asks the backend nothing about the
/// work browser and changes no control of it — a second road there would be a
/// second controller.
#[test]
fn the_panel_reads_who_holds_a_work_browser_and_moves_no_control_of_its_own() {
    let page = ui_file("shell-computer.js");
    for read in ["browserWorkspaceWords(", "watchBrowserWorkspace("] {
        assert!(
            page.contains(read),
            "the panel reads the dedicated browser through {read}"
        );
    }
    for road in [
        "\"set_browser_workspace_mode\"",
        "\"get_browser_workspaces\"",
        "\"browser_snapshot\"",
        "\"browser:workspace\"",
    ] {
        assert!(
            !page.contains(road),
            "shell-computer.js names {road}, a second road to the work browser's control"
        );
    }
    let browser = ui_file("shell-browser.js");
    assert!(
        block_after(&browser, "function paintBrowserWorkspaceChrome(")
            .contains("paintComputerProgressControl();"),
        "every repaint of the work browsers' chrome repaints the panel's control lines"
    );
}

#[test]
fn a_stop_is_lifted_on_either_road_only_by_the_one_resume() {
    let runtime = strip_rust_comments(RUNTIME);
    let settings = strip_rust_comments(SETTINGS);
    for (road, source) in [("an agent's resume", &runtime), ("the window", &settings)] {
        assert!(
            !source.contains("guard::lift()"),
            "{road} lifts no stop by itself"
        );
    }
    assert!(
        runtime.contains("computer_use::guard::Lifter::Agent"),
        "an agent's resume goes through the one resume"
    );
    let window = block_after(&settings, "pub(crate) fn computer_resume(");
    assert!(
        window.contains("computer_use::guard::Lifter::Person { revision }"),
        "the window's resume names the stop the band showed: {window}"
    );
}

#[test]
fn every_mark_press_passes_the_stop_check_in_its_one_door() {
    let marks = strip_rust_comments(MARKS);
    let made =
        marks.matches("PinnedClick {").count() - marks.matches("struct PinnedClick {").count();
    assert_eq!(made, 1, "one place makes a mark's press");
    let door = block_after(&marks, "pub(super) fn pinned_click_at(");
    let pressed = door
        .find("PinnedClick {")
        .expect("the press is made in pinned_click_at");
    let checked = door
        .find(".epoch")
        .expect("the press compares the stop its look began under");
    assert!(checked < pressed, "the stop is checked first: {door}");
    assert!(
        block_after(&marks, "pub fn pinned_click(")
            .contains("pinned_click_at(params, super::guard::stop_generation())"),
        "the window's press reads the stop count now"
    );
    assert!(
        !strip_rust_comments(RUNTIME).contains("pinned_click_at("),
        "the window presses only through the door that reads the stop count"
    );
    assert!(
        block_after(&marks, "fn latest_at(").contains(".epoch"),
        "a kept table serves only a look under the stop it was made under"
    );
}
