//! The work screen panel and the person's stop (t-59372): what the window
//! suite (`ui/tests/computer-progress.mjs`) and the backend's own tests
//! (`computer_use::guard`, `computer_use::observe::owned_tests`,
//! `computer_use::marks`) cannot see from inside one function. A look's
//! picture leaves the window's process only for the window's own page, only
//! about a pane the window holds and only for the assignment that pane still
//! holds; the page keeps one picture and no copy of it; a stop is lifted on
//! both roads by the one resume; no mark is pressed past a stop; and the
//! person's stop reaches the work browsers only by their own bar's road
//! (t-62872).

use std::path::Path;

use super::support::{block_after, strip_rust_comments};

const SETTINGS: &str = include_str!("../../src/cmd/settings.rs");
const RUNTIME: &str = include_str!("../../src/agent_tools_runtime.rs");
const MAIN: &str = include_str!("../../src/main.rs");
const OBSERVE: &str = include_str!("../../src/computer_use/observe.rs");
const MARKS: &str = include_str!("../../src/computer_use/marks.rs");
const GUARD: &str = include_str!("../../src/computer_use/guard.rs");
const COMPUTER_USE: &str = include_str!("../../src/computer_use/mod.rs");
const WORKSPACES: &str = include_str!("../../src/browser_workspace.rs");

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
/// and leads to that browser's own bar. The band's file asks the backend
/// nothing about a work browser — a second road there would be a second
/// controller; the person's stop moves one only through the browser's own
/// file (the next contract). The chrome reads the panel's painter from where
/// the band's file lays it when it loads (t-63505 ⑤), so the earlier file
/// never names a function a later one defines, and needs no guard for it.
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
            .contains("for (const read of browserWorkspaceChromeReaders) read();"),
        "every repaint of the work browsers' chrome repaints what reads them"
    );
    assert!(
        page.contains("browserWorkspaceChromeReaders.add("),
        "the band and the panel lay their painter where the chrome reads it"
    );
    for guarded in [
        "typeof paintComputerProgressControl",
        "typeof paintComputerBand",
    ] {
        assert!(
            !browser.contains(guarded),
            "shell-browser.js guards a call into a later file: {guarded}"
        );
    }
}

/// The person's one stop (t-62872): a stop the backend says is the person's
/// reaches the work browsers too, and each is paused, and given back on the
/// band's resume, by its own bar's road (`setBrowserWorkspaceMode`, which
/// names the revision the window holds). There is no second road:
/// shell-browser.js asks for a mode change in one place.
#[test]
fn the_persons_stop_reaches_the_work_browsers_only_through_their_bars_road() {
    let page = ui_file("shell-computer.js");
    let browser = ui_file("shell-browser.js");
    let take = block_after(&page, "function takeComputerBandStop(");
    // The condition the stop reaches the work browsers under, read in the
    // body: the signature names `persons` too, and a check of the whole block
    // would pass with the condition gone (t-62872).
    let (_, body) = take.split_once('\n').expect("the function has a body");
    let reach = body
        .lines()
        .find(|line| line.contains("stopAgentBrowserWorkspaces("))
        .unwrap_or_else(|| panic!("a granted stop reaches the work browsers: {take}"));
    assert!(
        reach.contains("&& persons"),
        "only a stop the backend says is the person's reaches them: {reach}"
    );
    assert!(
        page.contains("resumeAgentBrowserWorkspaces("),
        "the band's granted resume gives back what its stop paused"
    );
    assert_eq!(
        browser
            .matches("invoke(\"set_browser_workspace_mode\"")
            .count(),
        1,
        "one place asks the backend to change a work browser's mode"
    );
    for (opens, road) in [
        (
            "async function pauseBrowserWorkspaceForStop(",
            "setBrowserWorkspaceMode(tab, \"paused\"",
        ),
        (
            "function resumeAgentBrowserWorkspaces(",
            "setBrowserWorkspaceMode(tab, \"agent\"",
        ),
    ] {
        assert!(browser.contains(opens), "shell-browser.js has {opens}");
        let walk = block_after(&browser, opens);
        assert!(
            walk.contains(road),
            "{opens} goes by the bar's own road: {walk}"
        );
    }
}

/// The person's chord the helper heard (t-62872): the window takes it in
/// whenever it hears the helper's stop — at a `status`, at an agent's `stop`
/// before it stops, at a resume it refuses, at an action the helper refused —
/// unless the person's stop already stands here, and tells the band, so the
/// work browsers stop too once the window next hears from the helper. The
/// signed helper keeping the chord over a signal, and telling the window by
/// itself, is t-62469's.
#[test]
fn the_window_takes_in_the_persons_chord_whenever_it_hears_the_helper() {
    let runtime = strip_rust_comments(RUNTIME);
    let adopt = block_after(&runtime, "fn adopt_helper_stop(");
    let taken = adopt
        .find("computer_use::guard::adopt(")
        .unwrap_or_else(|| panic!("the window takes the helper's stop in: {adopt}"));
    let told = adopt
        .find("\"computer:activity\"")
        .unwrap_or_else(|| panic!("a stop taken in is told to the band: {adopt}"));
    assert!(taken < told, "told once taken in: {adopt}");
    let refused = "if error.code == error_code::STOPPED";
    let (at, _) = runtime
        .match_indices(refused)
        .find(|(at, _)| runtime[at + refused.len()..].trim_start().starts_with("&&"))
        .expect("the refused action's road");
    let road = &runtime[at..at
        + runtime[at..]
            .find("adopt_helper_stop(")
            .expect("it takes in")];
    assert!(
        road.contains("!computer_use::guard::persons_stop_stands()")
            && !road.contains("stopped_reason().is_none()"),
        "a refused action takes in the helper's stop unless the person's stands: {road}"
    );
    let standing = block_after(&runtime, "fn persons_stop_standing(");
    assert!(
        standing.contains("adopt_helper_stop("),
        "a refused resume takes in the stop it heard: {standing}"
    );
    let opens = "} else if command.method == ComputerMethod::Status {";
    let status = &runtime[runtime.find(opens).expect("the status road") + opens.len()..];
    let status = &status[..status.find("} else if").expect("the road ends")];
    assert!(
        status.contains("adopt_helper_stop(&helper, permission_window)"),
        "a status tells the band of a stop it took in: {status}"
    );
    // An agent's `stop` hears the helper before it stops (t-62872 r3): a
    // chord the window has not heard yet is taken in, so the stop that
    // follows keeps it the person's though its signal then overwrites the
    // helper's reason. It never waits behind a request in flight to hear
    // it: its signal is what ends that request at its next event. Both of
    // an agent's roads run on the window's helper.
    let stop = block_after(&runtime, "fn agents_stop(");
    // t-62469 F2: the window's stop stands first; the helper is heard by the
    // closure `stop_hearing` runs after it, and before the signal.
    let heard = stop
        .find("helper.status_now()")
        .unwrap_or_else(|| panic!("an agent's stop hears the helper: {stop}"));
    let stopped = stop
        .find("computer_use::guard::stop_hearing(")
        .expect("an agent's stop stops the window");
    assert!(
        stopped < heard && stop.contains("adopt_helper_stop(&heard, app)"),
        "the window's stop stands, then the helper's stop is heard and taken in: {stop}"
    );
    let session = block_after(&runtime, "impl HelperRoads for SessionHelper {");
    assert!(
        block_after(session, "fn status_now(").contains("computer_use::status_unless_busy()"),
        "the window's helper is heard now or not at all: {session}"
    );
    let busy = strip_rust_comments(COMPUTER_USE);
    let busy = block_after(&busy, "pub fn status_unless_busy(");
    assert!(
        busy.contains("held_client().try_lock()")
            && busy.contains("TryLockError::WouldBlock) => return None"),
        "a request in flight is not waited for: {busy}"
    );
    for road in [
        "Ok(agents_stop(&SessionHelper, permission_window))",
        "agents_resume(&SessionHelper, reset, permission_window)",
    ] {
        assert!(
            runtime.contains(road),
            "an agent's stop and resume take their one road: {road}"
        );
    }
}

/// `list` and `tabs` read each work browser by the door that reads the
/// person's stop (t-62872 r3): while it stands they skip a work browser, as
/// they skip one its stop paused, though they read only the window's own
/// records of it. No road of an agent's admits by `begin_at` alone.
#[test]
fn list_and_tabs_read_a_work_browser_only_by_the_door_that_reads_the_persons_stop() {
    let runtime = strip_rust_comments(RUNTIME);
    let dispatch = block_after(&runtime, "async fn dispatch_browser_command(");
    for arm in ["(\"list\", 1) => {", "(\"tabs\", 1) => {"] {
        let read = block_after(dispatch, arm);
        assert!(
            read.contains(".begin_agents(") && !read.contains(".begin_at("),
            "{arm} admits a work browser by the door that reads the person's stop: {read}"
        );
    }
    assert!(
        !runtime.contains(".begin_at("),
        "no road of an agent's admits by begin_at alone"
    );
}

/// The person's stop at the work browsers' door (t-62872): while it stands
/// the backend refuses an agent's `open` and every verb an agent sends to a
/// work browser, reading the window's own stop — never asking the helper —
/// first for its words, before any other refusal, and again where the door
/// admits, under that stop's own lock. The window's walk paints the bars and
/// records what the stop paused; the refusal does not wait for it.
#[test]
fn the_work_browsers_door_refuses_an_agent_while_the_persons_stop_stands() {
    let runtime = strip_rust_comments(RUNTIME);
    let answer = block_after(&runtime, "async fn answer_browser_command(");
    let said = answer
        .find("computer_use::guard::persons_stop_stands()")
        .unwrap_or_else(|| panic!("the door reads the person's stop: {answer}"));
    let actor = answer
        .find(".check_actor(pane, verb)")
        .expect("the door checks the actor");
    assert!(
        said < actor,
        "the person's stop is said before any other refusal: {answer}"
    );
    assert!(
        answer.contains(".begin_agents(") && !answer.contains(".begin_at("),
        "an agent's verb goes in only by the door that reads the stop: {answer}"
    );
    let dispatch = block_after(&runtime, "async fn dispatch_browser_command(");
    let open = &dispatch[dispatch.find("(\"open\", 2) =>").expect("the open arm")..];
    let open = &open[..open.find("(\"close\", 2) =>").expect("the close arm")];
    assert!(
        open.contains(".register_agents(&label, pane)") && !open.contains(".register(&label"),
        "an agent's open registers only by the door that reads the stop: {open}"
    );
    let workspaces = strip_rust_comments(WORKSPACES);
    for door in [
        "pub(crate) fn begin_agents(",
        "pub(crate) fn register_agents(",
        // What `tabs` says of a work browser (t-67064).
        "pub(crate) fn standing_agents(",
    ] {
        let body = block_after(&workspaces, door);
        assert!(
            body.contains("crate::computer_use::guard::unless_persons_stop("),
            "{door} admits under the person's stop's own lock: {body}"
        );
    }
    let guard = strip_rust_comments(GUARD);
    let unless = block_after(&guard, "pub fn unless_persons_stop<");
    assert!(
        !unless.contains("call(") && !unless.contains("helper"),
        "the door never asks the helper: {unless}"
    );
}

/// The person lifts their stop from the main window's band alone (t-62872):
/// the board pop-out's band offers the stop, which is safe from anywhere, and
/// not the resume, and the backend refuses a resume another page sends. A
/// stop pressed in the pop-out reaches the main window by its report, which
/// every page hears.
#[test]
fn the_persons_resume_is_the_main_windows_alone() {
    let settings = strip_rust_comments(SETTINGS);
    let resume = block_after(&settings, "pub(crate) fn computer_resume(");
    // The window's one check of which page asks (t-62872 r3), as its other
    // doors read it.
    let checked = resume
        .find("from_the_main_webview(&webview)")
        .unwrap_or_else(|| panic!("the resume checks which page asks: {resume}"));
    let lifted = resume
        .find("computer_use::guard::resume(")
        .expect("the resume");
    assert!(
        checked < lifted,
        "the page is checked before any stop is lifted: {resume}"
    );
    let stop = block_after(&settings, "pub(crate) fn computer_stop(");
    assert!(
        stop.contains("app.emit(") && !stop.contains("emit_to("),
        "every page hears the stop, the main window too: {stop}"
    );
    let sheet = ui_file("shell.css");
    for kept in [".computer-band-resume", ".computer-band-view"] {
        assert!(
            sheet.contains(&format!(":root[data-surface=\"popout\"] {kept},")),
            "the pop-out folds {kept} away"
        );
    }
}

/// Turning shared desktop input off in Settings stops the operator with the
/// window's own stop (t-62872): that is the person's stop, so it pauses every
/// work browser an agent holds as well — the safe side, kept on purpose.
#[test]
fn turning_shared_desktop_input_off_is_the_persons_stop_and_reaches_the_work_browsers() {
    use zerocode_core::computer_use::{STOP_REASON_WINDOW, persons_stop};
    let settings = strip_rust_comments(SETTINGS);
    let revoke = block_after(&settings, "pub(crate) fn set_computer_input_policy(");
    let stopped = revoke
        .find("computer_use::guard::stop(zerocode_core::computer_use::STOP_REASON_WINDOW)")
        .unwrap_or_else(|| {
            panic!("turning shared input off stops with the window's stop: {revoke}")
        });
    let told = revoke
        .find("\"computer:activity\"")
        .expect("and tells the band");
    assert!(stopped < told, "the band hears the stop it made: {revoke}");
    assert!(
        persons_stop(STOP_REASON_WINDOW),
        "the window's own stop is the person's"
    );
}

/// The guard's product code stands before its first test-only line (t-63505
/// ①): a reader that cuts a backend file at its first `#[cfg(test)]`, as
/// `shipped_backend` does, sees the whole guard — the stop, the report and the
/// one resume.
#[test]
fn the_guards_product_code_stands_before_its_first_test_only_line() {
    let (product, tests) = GUARD
        .split_once("#[cfg(test)]")
        .expect("the guard has its tests");
    for item in [
        "pub fn stop(",
        "pub fn activity_report(",
        "pub enum Lifter",
        "pub fn resume(",
    ] {
        assert!(
            product.contains(item) && !tests.contains(item),
            "{item} stands before the guard's first #[cfg(test)]"
        );
    }
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
