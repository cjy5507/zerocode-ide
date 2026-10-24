//! t-67936: 「계속 꺼졋어 갑자기」 — the person's coordinators went twice on
//! 2026-10-21 with no word on the screen. At 13:24:35 one tab the 12:37
//! restart had put back closed by close-tab with two runs' coordinators in it,
//! and six workers lost their coordinator; at 05:05 and 12:37 the window
//! restarted by its own restart road, at a gap nobody was watching for.
//!
//! Pinned here: a tab holding a seat asks before it goes whatever the running
//! switch says, and a close no hand made is refused; every close of a terminal
//! tab says its road in one black-box line; every restart stands its notice
//! first, held while the person types, and says its door's line; and the
//! window a restart started says which build it came back as.

use super::support::{block_after, shipped_backend, strip_rust_comments, window_source};

#[test]
fn a_tab_holding_a_seat_asks_first_and_refuses_a_close_no_hand_made() {
    let window = window_source();
    let closing = block_after(window, "function closeTab(id) {");
    // The seats are asked about before the running switch is read, so the
    // person's 「도는 터미널에 대해 다시 묻지 않기」 cannot silence them.
    let seats = closing
        .find("if (holdsSeat(held)) {")
        .unwrap_or_else(|| panic!("the tab close no longer asks about seats:\n{closing}"));
    let running = closing
        .find("if (confirmCloseRunning) {")
        .unwrap_or_else(|| panic!("the running question left the tab close:\n{closing}"));
    assert!(
        seats < running,
        "the running switch is read before the seats:\n{closing}"
    );
    for needle in [
        "const road = closeRoadNow();",
        "const held = tabHoldings(tab);",
        "if (road.synthetic) {\n          refuseSyntheticClose(tab, road, held);\n          return;\n        }",
        "askAboutSeats(held).then((proceed) => passCloseCheck(tab, road, held, true, proceed))",
        "noteCloseRoad(tab, road, held, \"close requested\");",
    ] {
        assert!(
            closing.contains(needle),
            "the tab close lost `{needle}`:\n{closing}"
        );
    }
    let holdings = block_after(window, "function tabHoldings(tab) {");
    assert!(
        holdings.contains("paneRole(term)")
            && holdings.contains("storedLayoutsHoldProgram([tab.asleep])")
            && holdings.contains("termHoldsAgent(term)"),
        "what a tab holds is read from somewhere other than the ledger's seats \
         and the window's agents:\n{holdings}"
    );
    let asking = block_after(window, "async function askAboutSeats(held) {");
    assert!(
        asking.contains("terminal.holdsTitle")
            && asking.contains("danger: true")
            && !asking.contains("remember"),
        "the seat question lost its words, its weight, or grew a way to stop \
         asking:\n{asking}"
    );
}

#[test]
fn every_close_of_a_terminal_tab_says_its_road_and_whose_hand() {
    let window = window_source();
    let road = block_after(window, "function closeRoadNow() {");
    assert!(
        road.contains("window.event")
            && road.contains("handSaid()")
            && road.contains("event.isTrusted")
            && road.contains("event.clientX"),
        "a close no longer reads its road, its hand or its pointer off the \
         event:\n{road}"
    );
    let hand = block_after(window, "function handSaid() {");
    assert!(
        hand.contains("event?.isTrusted")
            && hand.contains("`synthetic ${event.type}`")
            && hand.contains("handMoved"),
        "the hand a door was moved by is no longer told apart from a script's \
         event:\n{hand}"
    );
    let line = block_after(window, "function noteCloseRoad(tab, road, held, outcome) {");
    assert!(
        line.contains("invoke(\"log_window_error\"")
            && line.contains("computerActingWord()")
            && line.contains("seat.run")
            && line.contains("facts.worker"),
        "a close's black-box line lost its road, Computer Use or the seats it \
         held:\n{line}"
    );
    // Ids and counts only: a run's name, a tab's title and a prompt are the
    // person's words, and stay out of the log.
    assert!(
        !line.contains("seat.name") && !line.contains("tabLabel(") && !line.contains("panePrompts"),
        "a close's black-box line carries the person's words:\n{line}"
    );
}

#[test]
fn every_restart_stands_its_notice_first_and_says_its_door() {
    let backend = shipped_backend();
    let main = strip_rust_comments(include_str!("../../src/main.rs"));
    assert!(
        !main.contains("            relaunch_window,\n"),
        "the restart road is a command again — a window could restart with no \
         notice on the screen"
    );
    for command in ["leave_hold", "boot_after_restart"] {
        assert!(
            main.contains(&format!("            {command},\n")),
            "{command} is not registered in generate_handler!"
        );
    }
    let road = block_after(backend, "pub(crate) fn relaunch_window(");
    let at = |needle: &str| {
        road.find(needle)
            .unwrap_or_else(|| panic!("the restart road lost `{needle}`:\n{road}"))
    };
    assert!(
        at("crate::exit_runtime::door_line(road)") < at("app.restart()")
            && at("crate::exit_runtime::leave_restart_note(&root, &note)") < at("app.restart()"),
        "the restart goes before it says its door or leaves its note:\n{road}"
    );
    // And every other road out says whose hand moved it, once per exit.
    let tray = include_str!("../../src/native_tray.rs");
    for (source, opens) in [
        (
            main.as_str(),
            "tauri::RunEvent::ExitRequested { api, code, .. } => {",
        ),
        (main.as_str(), "tauri::RunEvent::Exit => {"),
        (tray, "QUIT_ID => {"),
    ] {
        let arm = block_after(source, opens);
        assert!(
            arm.contains("exit_runtime::door_line(road)"),
            "a road out of the window says no door line:\n{arm}"
        );
    }
    let exiting = include_str!("../../src/exit_runtime.rs");
    let going = block_after(
        exiting,
        "fn go(state: &mut Exiting, asking: Asking, choice: Choice, now_ms: i64) -> ExitBeat {",
    );
    assert!(
        going.contains("asking.patience().notice_ms") && going.contains("noticed(state, now_ms)"),
        "a decided road no longer raises the table's notice:\n{going}"
    );
    let step = block_after(exiting, "fn step(");
    assert!(
        step.contains("return noticed(state, now_ms);") && !step.contains("settle("),
        "a road the beat decides goes with no notice:\n{step}"
    );
    let beat = block_after(backend, "pub(crate) fn beat_leaving(");
    assert!(
        beat.contains("\"exit:notice\""),
        "the beat no longer tells the window its notice:\n{beat}"
    );
    let window = window_source();
    for needle in [
        "listen(\"exit:notice\"",
        "invoke(\"leave_hold\"",
        "invoke(\"boot_after_restart\")",
        "standExitNotice(await invoke(\"leave_now\", { road, door, by }))",
        "invoke(\"leave_when_idle\", { road, door, by })",
    ] {
        assert!(
            window.contains(needle),
            "the window lost `{needle}` — a restart it shows no notice for, or \
             one it does not say it came back from"
        );
    }
    // The window a restart started says so as it comes up: on the first
    // settings snapshot, once.
    let booting = block_after(window, "function applyUpdateSettingsSnapshot(");
    assert!(
        booting.contains("if (first) void sayRestartBack();"),
        "the window no longer says which build a restart brought it back as:\n{booting}"
    );
}
