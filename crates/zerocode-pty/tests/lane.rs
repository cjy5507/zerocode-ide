//! The terminal surface against a real child process, plus the independence
//! contract: the IDE must work on a machine with no `zo` installed.

use std::path::Path;
#[cfg(unix)]
use std::time::{Duration, Instant};

use zerocode_pty::ZoBinary;
#[cfg(unix)]
use zerocode_pty::{PTY_OUTPUT_BYTE_BUDGET, PtyLane, ZO_EXECUTABLE};

/// Pump until `predicate` holds or the deadline passes. Returns whether it held.
#[cfg(unix)]
fn pump_until(lane: &mut PtyLane, predicate: impl Fn(&PtyLane) -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        lane.pump();
        if predicate(lane) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    false
}

/// Pump for `patience` whatever happens.
///
/// For a claim about what did NOT arrive, which no predicate can end early:
/// [`pump_until`] would return the moment its condition held and prove
/// nothing about the milliseconds after it.
#[cfg(unix)]
fn pump_for(lane: &mut PtyLane, patience: Duration) {
    let deadline = Instant::now() + patience;
    while Instant::now() < deadline {
        lane.pump();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Ask the operating system, not the lane, whether a process is still there.
#[cfg(unix)]
fn process_is_alive(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string()])
        .output()
        .expect("run ps")
        .status
        .success()
}

/// A lane owns its child. Letting the lane fall out of scope has to take the
/// process with it, or every closed window leaves an agent running — spending
/// tokens to draw a screen nobody can ever read again.
///
/// This is the whole class: without it, dropping a registry of eight lanes
/// leaks eight, and handing a lane somewhere that refuses it leaks that one,
/// because the caller already moved it and has no handle left.
#[cfg(unix)]
#[test]
fn dropping_a_lane_takes_its_child_with_it() {
    let lane = PtyLane::spawn(
        "/bin/sh",
        &["-c".to_string(), "sleep 30".to_string()],
        None,
        &[],
        6,
        20,
    )
    .expect("spawn");

    let pid = lane.pid().expect("a running child has a pid");
    assert!(process_is_alive(pid), "the child should be running");

    drop(lane);

    // Reaping is not instant; give the signal a moment to land.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && process_is_alive(pid) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !process_is_alive(pid),
        "pid {pid} survived its lane being dropped"
    );
}

#[cfg(unix)]
#[test]
fn a_child_process_paints_the_grid() {
    let mut lane = PtyLane::spawn(
        "/bin/sh",
        &["-c".to_string(), "printf 'hello from the lane'".to_string()],
        None,
        &[],
        10,
        40,
    )
    .expect("spawn");

    assert!(
        pump_until(&mut lane, |lane| {
            lane.terminal()
                .grid()
                .visible_text()
                .contains("hello from the lane")
        }),
        "child output never reached the grid; saw {:?}",
        lane.terminal().grid().visible_text()
    );
}

#[cfg(unix)]
#[test]
fn input_reaches_the_child_and_its_answer_comes_back() {
    let mut lane = PtyLane::spawn(
        "/bin/sh",
        &[
            "-c".to_string(),
            "read line; printf 'got:%s' \"$line\"".to_string(),
        ],
        None,
        &[],
        10,
        40,
    )
    .expect("spawn");

    lane.write_input(b"drain-gate\n").expect("write");
    assert!(
        pump_until(&mut lane, |lane| {
            lane.terminal()
                .grid()
                .visible_text()
                .contains("got:drain-gate")
        }),
        "the child never echoed our input; saw {:?}",
        lane.terminal().grid().visible_text()
    );
}

#[cfg(unix)]
#[test]
fn a_finished_child_reports_its_exit_code_and_ends() {
    let mut lane = PtyLane::spawn(
        "/bin/sh",
        &["-c".to_string(), "exit 3".to_string()],
        None,
        &[],
        6,
        20,
    )
    .expect("spawn");

    assert!(
        pump_until(&mut lane, PtyLane::has_ended),
        "the lane never observed the child closing"
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    let mut code = None;
    while Instant::now() < deadline && code.is_none() {
        code = lane.try_wait().expect("try_wait");
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(code, Some(3));
}

#[cfg(unix)]
#[test]
fn resizing_a_live_lane_resizes_the_grid_too() {
    let mut lane = PtyLane::spawn("/bin/cat", &[], None, &[], 10, 40).expect("spawn");

    lane.resize(24, 100).expect("resize");
    assert_eq!(lane.terminal().grid().rows(), 24);
    assert_eq!(lane.terminal().grid().cols(), 100);

    lane.kill().expect("kill");
}

/// A resize to the size a lane already has is nothing at all, and says
/// nothing.
///
/// The window asks whenever a screen COULD have changed shape — a stage
/// repaint, a divider, a tab coming forward — and hardly any of those change
/// one: bringing a tab forward asks for the grid it already had. Two things
/// must not happen on that road. The pty must not be told (an `ioctl` per
/// visible shell, per switch, to hand the kernel a size it already knows),
/// and no frame may come out of it — a frame from a resize is a FULL frame,
/// so a screen that answered here would have every terminal on the stage
/// repainting itself every time somebody clicked a tab.
#[cfg(unix)]
#[test]
fn a_resize_to_the_size_it_already_has_says_nothing() {
    let mut lane = PtyLane::spawn("/bin/cat", &[], None, &[], 10, 40).expect("spawn");
    // What a newborn lane owes anybody, taken first so the next answer is
    // about the resize and nothing else.
    lane.terminal_mut().grid_mut().take_delta();

    lane.resize(10, 40).expect("resize");
    assert!(
        lane.terminal_mut().grid_mut().take_delta().is_none(),
        "a resize to the size it already had produced a frame",
    );

    // And a real one still redraws, or the screen keeps wrapping at the width
    // it no longer has.
    lane.resize(24, 100).expect("resize");
    let delta = lane
        .terminal_mut()
        .grid_mut()
        .take_delta()
        .expect("a resize that changes the size produces a frame");
    assert!(delta.full, "a reflowed screen is redrawn whole");
    assert_eq!(delta.size, (24, 100));

    lane.kill().expect("kill");
}

#[cfg(unix)]
#[test]
fn the_environment_we_pass_reaches_the_child() {
    let mut lane = PtyLane::spawn(
        "/bin/sh",
        &[
            "-c".to_string(),
            "printf 'key=%s' \"$ZEROCODE_PANE_KEY\"".to_string(),
        ],
        None,
        &[("ZEROCODE_PANE_KEY".to_string(), "tab-1/leaf-1".to_string())],
        6,
        40,
    )
    .expect("spawn");

    assert!(
        pump_until(&mut lane, |lane| {
            lane.terminal()
                .grid()
                .visible_text()
                .contains("key=tab-1/leaf-1")
        }),
        "the pane key never reached the child; saw {:?}",
        lane.terminal().grid().visible_text()
    );
}

// ---------------------------------------------------------------- zo discovery

#[test]
fn a_machine_without_zo_reports_absence_rather_than_failing() {
    let empty = tempfile::tempdir().expect("tempdir");
    let path = std::ffi::OsString::from(empty.path());
    assert_eq!(ZoBinary::discover_in(Some(&path)), None);
    assert_eq!(ZoBinary::discover_in(None), None);
}

#[cfg(unix)]
#[test]
fn zo_is_found_on_path_when_it_is_installed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let binary = dir.path().join(ZO_EXECUTABLE);
    std::fs::write(&binary, "#!/bin/sh\nexit 0\n").expect("write");
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&binary).expect("metadata").permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&binary, perms).expect("chmod");
    }

    let path = std::ffi::OsString::from(dir.path());
    let found = ZoBinary::discover_in(Some(&path)).expect("zo should be found");
    assert_eq!(found.path, binary);
}

#[cfg(unix)]
#[test]
fn a_non_executable_file_named_zo_is_not_mistaken_for_the_binary() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join(ZO_EXECUTABLE), "notes, not a binary").expect("write");
    let path = std::ffi::OsString::from(dir.path());
    assert_eq!(ZoBinary::discover_in(Some(&path)), None);
}

#[test]
fn the_attach_command_targets_a_session_on_a_running_server() {
    let args = ZoBinary::attach_args(Some("session-1780878278884-0"), "127.0.0.1:8787");
    assert_eq!(
        args,
        vec![
            "attach",
            "session-1780878278884-0",
            "--bind",
            "127.0.0.1:8787"
        ]
    );
    assert_eq!(
        ZoBinary::serve_args("127.0.0.1:8787"),
        vec!["serve", "--bind", "127.0.0.1:8787"]
    );
}

/// `zo attach [SESSION_ID]` creates a session only when none is named, so a new
/// lane must **omit** the argument. Substituting a placeholder attaches to a
/// session by that literal name, which the server never registers — the lane
/// renders anyway, so the mistake is invisible until someone looks for their
/// work and `session.list` is empty.
#[test]
fn a_new_session_omits_the_id_rather_than_inventing_one() {
    let args = ZoBinary::attach_args(None, "127.0.0.1:8787");
    assert_eq!(args, vec!["attach", "--bind", "127.0.0.1:8787"]);
    assert!(
        !args.iter().any(|arg| arg == "new"),
        "a placeholder id would name a session instead of creating one: {args:?}"
    );
}

/// The IDE pane is one `zo` process with one events channel. It does not use
/// the legacy attach subcommand, and resuming always carries the requested id
/// immediately after `--resume` (bare `--resume` only lists sessions).
#[test]
fn the_pane_command_opens_an_events_channel_without_attach() {
    let args = ZoBinary::pane_args("127.0.0.1:0", Some(Path::new("/w/repo")), None);
    assert_eq!(
        args,
        vec!["--events-bind", "127.0.0.1:0", "--cwd", "/w/repo"]
    );
    assert!(!args.iter().any(|arg| arg == "attach"));

    assert_eq!(
        ZoBinary::pane_args("127.0.0.1:8788", None, Some("session-42")),
        vec!["--events-bind", "127.0.0.1:8788", "--resume", "session-42"]
    );
}

/// A question asked by the program reaches the terminal and the answer gets
/// back to it.
///
/// The grid can queue an answer, but a queue nobody drains is the same as no
/// answer at all — the child sits waiting. This is the wire between them.
#[cfg(unix)]
#[test]
fn a_query_from_the_child_is_answered_through_the_pty() {
    let mut lane = PtyLane::spawn(
        "sh",
        &["-c".to_string(), "printf '\\033[c'; sleep 2".to_string()],
        None,
        &[],
        10,
        40,
    )
    .expect("a shell");

    // Pump until the query has been seen and answered, or give up.
    let mut answered = false;
    for _ in 0..200 {
        lane.pump();
        if lane.terminal().grid().cursor() != (0, 0) || answered {
            break;
        }
        answered = true;
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // The reply queue must be empty: it was drained into the child rather
    // than left to be sent twice.
    assert!(
        lane.terminal_mut().grid_mut().take_replies().is_empty(),
        "an answer was queued but never sent to the program that asked"
    );
}

/* ---- the marks of the session that started us go no further ---- */

#[cfg(unix)]
#[test]
fn a_child_does_not_inherit_the_session_markers_of_whatever_started_this_window() {
    // The reported symptom, reproduced from the outside: this window is very
    // often started FROM an agent, a pty inherits the environment of whatever
    // spawned it, and so every shell it opened was handed the marks of that
    // session. The agent inside then read them as its own —
    // `inherited CLAUDE_CODE_CHILD_SESSION marker`.
    //
    // Set on THIS process, which is exactly how they arrive in the real case.
    for marker in zerocode_pty::INHERITED_SESSION_MARKERS {
        // SAFETY: single-threaded at this point in the test, and the value is
        // read back only through the child that is spawned below.
        unsafe { std::env::set_var(marker, "from-the-parent") };
    }

    let mut lane = PtyLane::spawn(
        "/bin/sh",
        &[
            "-c".to_string(),
            // `-` for absent, so the assertion can tell "empty" from "gone" —
            // the two differ for anything that tests presence, which is what
            // every one of these markers is tested for.
            "printf 'child=[%s][%s][%s][%s]' \
             \"${CLAUDE_CODE_CHILD_SESSION:--}\" \"${CLAUDECODE:--}\" \
             \"${CLAUDE_CODE_SSE_PORT:--}\" \"${CLAUDE_CODE_ENTRYPOINT:--}\""
                .to_string(),
        ],
        None,
        &[],
        6,
        80,
    )
    .expect("spawn");

    assert!(
        pump_until(&mut lane, |lane| lane
            .terminal()
            .grid()
            .visible_text()
            .contains("child=")),
        "the child never reported; saw {:?}",
        lane.terminal().grid().visible_text()
    );
    let said = lane.terminal().grid().visible_text();
    assert!(
        said.contains("child=[-][-][-][-]"),
        "a session marker was handed down to the child: {said:?}"
    );

    lane.kill().expect("kill");
    for marker in zerocode_pty::INHERITED_SESSION_MARKERS {
        // SAFETY: as above.
        unsafe { std::env::remove_var(marker) };
    }
}

#[cfg(unix)]
#[test]
fn a_launch_that_sets_a_marker_on_purpose_still_wins() {
    // The other half of the rule, and the half a blind delete list gets wrong.
    // Orca strips only the keys the spawn env does not itself set
    // (`getInheritedClaudeSessionStampEnvKeysToDelete`) — because this window
    // DOES set one of these on purpose: launching a team of agents needs
    // `CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS`, and a strip that ignored the
    // caller would take it back out of the one launch that meant it.
    // SAFETY: single-threaded, and read back only through the child.
    unsafe { std::env::set_var("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS", "from-the-parent") };

    let mut lane = PtyLane::spawn(
        "/bin/sh",
        &[
            "-c".to_string(),
            "printf 'teams=[%s]' \"${CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS:--}\"".to_string(),
        ],
        None,
        &[(
            "CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS".to_string(),
            "1".to_string(),
        )],
        6,
        40,
    )
    .expect("spawn");

    assert!(
        pump_until(&mut lane, |lane| lane
            .terminal()
            .grid()
            .visible_text()
            .contains("teams=")),
        "the child never reported; saw {:?}",
        lane.terminal().grid().visible_text()
    );
    let said = lane.terminal().grid().visible_text();
    assert!(
        said.contains("teams=[1]"),
        "the strip took out a marker the launch asked for: {said:?}"
    );

    lane.kill().expect("kill");
    // SAFETY: as above.
    unsafe { std::env::remove_var("CLAUDE_CODE_EXPERIMENTAL_AGENT_TEAMS") };
}

#[cfg(unix)]
#[test]
fn a_pane_names_this_terminal_not_the_one_that_opened_the_window() {
    // Measured 2026-09-17: the window was opened from Terminal.app, so every
    // pane was handed `TERM_PROGRAM=Apple_Terminal` and that tab's session id.
    // Claude Code 2.1.273 in fullscreen believed it and polled `CSI ? 6 n`
    // every 200 ms to catch a Cmd+K clear this window cannot send; the grid
    // answered each poll, so an idle pane wrote five times a second forever,
    // never went quiet, and every launch briefing and mail pointer waiting on
    // that quiet timed out. zsh read the same name and ran Terminal.app's
    // session script against the one inherited session id in every pane.
    //
    // Staged on THIS process, which is how they arrive in the real case.
    for (name, value) in [
        ("TERM_PROGRAM", "Apple_Terminal"),
        ("TERM_PROGRAM_VERSION", "466"),
    ] {
        // SAFETY: single-threaded at this point in the test, and the values
        // are read back only through the child that is spawned below.
        unsafe { std::env::set_var(name, value) };
    }
    for marker in zerocode_pty::INHERITED_TERMINAL_IDENTITY {
        // SAFETY: as above.
        unsafe { std::env::set_var(marker, "from-the-parent") };
    }

    let mut lane = PtyLane::spawn(
        "/bin/sh",
        &[
            "-c".to_string(),
            "printf 'program=[%s][%s]\\n' \"${TERM_PROGRAM:--}\" \"${TERM_PROGRAM_VERSION:--}\"; \
             for name in \"$@\"; do eval \"printf '%s' \\\"\\${$name+inherited:$name }\\\"\"; done; \
             printf 'identity-done'"
                .to_string(),
            "sh".to_string(),
        ]
        .into_iter()
        .chain(
            zerocode_pty::INHERITED_TERMINAL_IDENTITY
                .iter()
                .map(|marker| (*marker).to_string()),
        )
        .collect::<Vec<_>>(),
        None,
        &[],
        24,
        200,
    )
    .expect("spawn");

    assert!(
        pump_until(&mut lane, |lane| lane
            .terminal()
            .grid()
            .visible_text()
            .contains("identity-done")),
        "the child never reported; saw {:?}",
        lane.terminal().grid().visible_text()
    );
    let said = lane.terminal().grid().visible_text();
    lane.kill().expect("kill");
    for name in ["TERM_PROGRAM", "TERM_PROGRAM_VERSION"]
        .into_iter()
        .chain(zerocode_pty::INHERITED_TERMINAL_IDENTITY.iter().copied())
    {
        // SAFETY: as above.
        unsafe { std::env::remove_var(name) };
    }

    let declared = format!(
        "program=[{}][{}]",
        zerocode_pty::TERMINAL_NAME,
        zerocode_pty::TERMINAL_VERSION
    );
    assert!(
        said.contains(&declared),
        "the pane introduced itself as the terminal that opened the window: {said:?}"
    );
    assert!(
        !said.contains("inherited:"),
        "another terminal's session identity reached the child: {said:?}"
    );
}

#[cfg(unix)]
#[test]
fn our_own_hook_and_mirror_variables_are_untouched() {
    // The strip must be narrow. A lane carries its pane key, its hook token
    // and an account's config home to the child, and those are the whole
    // reason `env` exists — a rule that reached them would break every agent
    // launch in the window.
    let mut lane = PtyLane::spawn(
        "/bin/sh",
        &[
            "-c".to_string(),
            "printf 'ours=[%s][%s]' \"${ZEROCODE_PANE_KEY:--}\" \"${CLAUDE_CONFIG_DIR:--}\""
                .to_string(),
        ],
        None,
        &[
            ("ZEROCODE_PANE_KEY".to_string(), "tab-1/leaf-1".to_string()),
            ("CLAUDE_CONFIG_DIR".to_string(), "/tmp/home".to_string()),
        ],
        6,
        60,
    )
    .expect("spawn");

    assert!(
        pump_until(&mut lane, |lane| lane
            .terminal()
            .grid()
            .visible_text()
            .contains("ours=")),
        "the child never reported; saw {:?}",
        lane.terminal().grid().visible_text()
    );
    assert!(
        lane.terminal()
            .grid()
            .visible_text()
            .contains("ours=[tab-1/leaf-1][/tmp/home]"),
        "the strip reached a variable this window sets on purpose: {:?}",
        lane.terminal().grid().visible_text()
    );

    lane.kill().expect("kill");
}

/// A job-control shell hands the terminal to what it runs and takes it back
/// when that ends — and the lane can read which of the two is true right now,
/// without asking the program to say so.
///
/// This is the fact a hookless agent's exit is detected by: the agent held the
/// terminal, the shell it ran in did not die with it, and the ONLY thing that
/// changed in the operating system is which process group the pty is pointed
/// at. Measured against a real shell and a real child, because the whole claim
/// is about kernel behaviour that no in-process fake could get wrong in the
/// same way.
#[cfg(unix)]
#[test]
fn the_lane_can_tell_a_shell_at_its_prompt_from_one_running_a_job() {
    // `-m` is job control in a non-interactive shell: without it the shell
    // never moves the terminal at all and the answer would be "the child" for
    // its whole life. An interactive shell turns this on for itself, which is
    // exactly the case this exists for.
    let mut lane =
        PtyLane::spawn("/bin/sh", &["-m".to_string()], None, &[], 10, 40).expect("spawn");
    let shell_pid = lane.pid().expect("shell pid");

    assert_eq!(
        wait_for_foreground(&mut lane, true),
        Some(true),
        "a shell sitting at its own prompt is not holding its own terminal"
    );
    assert_eq!(lane.foreground_process_id(), Some(shell_pid));

    lane.write_input(b"sleep 4\n").expect("write");
    assert_eq!(
        wait_for_foreground(&mut lane, false),
        Some(false),
        "the shell never handed the terminal to the job it started"
    );
    assert!(
        lane.foreground_process_id()
            .is_some_and(|pid| pid != shell_pid),
        "the foreground pid still names the shell instead of its child job"
    );

    // And back, when the job ends — the edge the pump reads as "whatever was
    // running in here is gone".
    assert_eq!(
        wait_for_foreground(&mut lane, true),
        Some(true),
        "the shell never took the terminal back after its job ended"
    );
    assert_eq!(lane.foreground_process_id(), Some(shell_pid));
}

/// One byte ends a job, and it is the kernel that ends it.
///
/// Reported live: Ctrl+C in a terminal did nothing. The window was at fault —
/// a control chord under a non-Latin input source arrived as the layout's
/// glyph and went out as that glyph — but the report could equally have meant
/// this half, and nothing here said otherwise. So it says so now, against a
/// real shell holding a real job: writing ETX to the master makes the line
/// discipline signal the foreground group, and the shell takes its terminal
/// back. No fake can be wrong about this in the same way, because the claim
/// is entirely about what the kernel does with one byte.
#[cfg(unix)]
#[test]
fn one_byte_gives_the_shell_its_terminal_back() {
    // `-m` is job control in a non-interactive shell — see the test above for
    // why a shell without it never hands the terminal over at all.
    let mut lane =
        PtyLane::spawn("/bin/sh", &["-m".to_string()], None, &[], 10, 40).expect("spawn");
    assert_eq!(
        wait_for_foreground(&mut lane, true),
        Some(true),
        "a shell sitting at its own prompt is not holding its own terminal"
    );

    lane.write_input(b"sleep 30\n").expect("write");
    assert_eq!(
        wait_for_foreground(&mut lane, false),
        Some(false),
        "the shell never handed the terminal to the job it started"
    );

    lane.write_input(b"\x03").expect("write the interrupt");
    assert_eq!(
        wait_for_foreground(&mut lane, true),
        Some(true),
        "ETX reached the pty and the job it should have interrupted is still there"
    );
}

/// One flooding lane cannot have the whole round.
///
/// The pump runs on the thread that draws every other pane, and a child can
/// produce faster than a grid can parse — so a drain that stops only when the
/// channel runs dry stops when the CHILD decides, not when we do. That is the
/// same shape as the freeze above seen from the other direction: one pane's
/// traffic, everybody's window.
///
/// A bounded flood on purpose: an endless one would hang rather than fail if
/// this ever regresses, and a test that hangs reports nothing to anybody.
#[cfg(unix)]
#[test]
fn one_pump_takes_a_round_of_a_flood_and_not_the_whole_thing() {
    let mut lane = PtyLane::spawn(
        "/bin/sh",
        &[
            "-c".to_string(),
            "head -c 2000000 /dev/zero | tr '\\0' x".to_string(),
        ],
        None,
        &[],
        10,
        40,
    )
    .expect("spawn");

    // Let the reader thread get well ahead of the grid, which is the state
    // this is about — with nothing queued, any pump is a short one and the
    // bound would go untested.
    std::thread::sleep(Duration::from_millis(250));

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut moved = 0;
    let mut largest = 0;
    while Instant::now() < deadline && moved < PTY_OUTPUT_BYTE_BUDGET * 4 {
        let pumped = lane.pump();
        largest = largest.max(pumped.bytes);
        moved += pumped.bytes;
        if pumped.bytes == 0 {
            if pumped.ended {
                break;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    assert!(
        moved > PTY_OUTPUT_BYTE_BUDGET,
        "the flood never arrived, so nothing here was measured: {moved} bytes"
    );
    assert!(
        largest <= PTY_OUTPUT_BYTE_BUDGET,
        "one pump moved {largest} bytes, and every other pane waited for it"
    );
}

/// A child that has stopped reading cannot hold the thread that types at it.
///
/// Reported live twice inside one hour as a crash. It was not one: the window
/// was parked in `write` inside `write_input` for the whole of a two-second
/// sample, every pane frozen behind it, because a pty write waits for room
/// when the child is not making any. One child pausing between reads took
/// eight lanes and the window down with it.
///
/// Raw mode is not decoration here, it is the only state in which this is
/// reproducible AND the state every hosted agent puts its terminal into: a
/// canonical line discipline DISCARDS input it has no room for, so it never
/// makes anyone wait. Measured on this machine before the fix — nine
/// megabytes went into a canonical-mode `sleep` without a pause, and not one
/// byte of it arrived.
///
/// The write runs on a thread of its own for one reason: before the fix it
/// never returns, and a test that hangs reports nothing to anybody.
#[cfg(unix)]
#[test]
fn a_child_that_stopped_reading_does_not_take_the_typist_with_it() {
    // `stty raw` and then a child that reads nothing at all — an agent
    // thinking rather than draining, held still. 30s outlasts the deadline
    // below, so the child cannot end the wait by exiting and pass this for
    // the wrong reason.
    let mut lane = PtyLane::spawn(
        "/bin/sh",
        &[
            "-c".to_string(),
            "stty raw -echo; echo READY; exec sleep 30".to_string(),
        ],
        None,
        &[],
        10,
        40,
    )
    .expect("spawn");
    assert!(
        pump_until(&mut lane, |lane| lane
            .terminal()
            .grid()
            .visible_text()
            .contains("READY")),
        "the child never reached raw mode, so nothing below would prove anything"
    );

    // A megabyte is far past the line discipline's own room, which is counted
    // in single kilobytes — so every one of these would have waited.
    let flood = vec![b'x'; 1024 * 1024];
    let (report, answered) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let took_it = lane.write_input(&flood).is_ok();
        let refused = (0..8).any(|_| lane.write_input(&flood).is_err());
        let _ = report.send((took_it, refused));
    });

    assert_eq!(
        answered.recv_timeout(Duration::from_secs(5)),
        Ok((true, true)),
        "typing at a child that is not reading either held the caller or \
         queued without limit"
    );
}

/// Pump until the lane's foreground answer is `want`, and report what it was.
#[cfg(unix)]
fn wait_for_foreground(lane: &mut PtyLane, want: bool) -> Option<bool> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut seen = lane.foreground_is_child();
    while Instant::now() < deadline {
        lane.pump();
        seen = lane.foreground_is_child();
        if seen == Some(want) {
            return seen;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    seen
}

/// An answer reaches the child that asked, and never the pane.
///
/// A cooked pty copies whatever is written to it straight back out, so the
/// answer to `CSI 6n` asked before a program arms raw mode was painted on the
/// pane as `^[[1;1R` and spliced into whatever was typed next — the original
/// hit exactly this and `gh auth login` died on the leftovers (#12112,
/// #13137). The answer still goes out in the same turn; its echo is taken
/// back out of the output it comes home in.
///
/// The nap is what makes this deterministic: it puts the write squarely
/// inside the cooked window, so the echo definitely happens and definitely
/// has to be recognised.
#[cfg(unix)]
#[test]
fn an_answer_reaches_the_child_that_asked_and_never_the_pane() {
    let mut lane = PtyLane::spawn(
        "/bin/sh",
        &[
            "-c".to_string(),
            "printf '\\033[6n'; sleep 0.2; stty raw -echo; \
             dd bs=1 count=6 >/dev/null 2>&1; printf READY; exec sleep 5"
                .to_string(),
        ],
        None,
        &[],
        10,
        40,
    )
    .expect("spawn");
    // READY is printed only after six bytes were read back, so this waiting
    // for it is the proof that the answer arrived at all.
    assert!(
        pump_until(&mut lane, |lane| lane
            .terminal()
            .grid()
            .visible_text()
            .contains("READY")),
        "the answer never reached the child that asked for it"
    );
    let screen = lane.terminal().grid().visible_text();
    assert!(
        !screen.contains("^[") && !screen.contains("1;1R"),
        "the answer's own echo was drawn on the pane:\n{screen}"
    );
}

/// And on a tty that never leaves cooked mode either.
///
/// This is `printf '\033[6n'` typed at a shell prompt: nobody is going to
/// clear ECHO, so the echo is certain and waiting for quiet would have been
/// waiting forever. Recognising the shape is the only thing that works here,
/// which is why it does the work on the other path too.
#[cfg(unix)]
#[test]
fn an_echo_on_a_tty_that_stays_cooked_is_not_drawn_either() {
    let mut lane = PtyLane::spawn(
        "/bin/sh",
        &[
            "-c".to_string(),
            "printf '\\033[6n'; exec sleep 5".to_string(),
        ],
        None,
        &[],
        10,
        40,
    )
    .expect("spawn");
    // Long enough for the answer, the echo and a few rounds after both.
    pump_for(&mut lane, Duration::from_millis(400));
    let screen = lane.terminal().grid().visible_text();
    assert!(
        !screen.contains("^[") && !screen.contains("1;1R"),
        "the answer's own echo was drawn on the pane:\n{screen}"
    );
}
