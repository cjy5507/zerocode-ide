use std::path::Path;

#[test]
fn crash_report_has_bounded_memory_and_a_boot_door() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    for name in [
        "crash.rs",
        "crumbs.rs",
        "hang_watchdog.rs",
        "hang_sample.rs",
    ] {
        assert!(src.join(name).is_file(), "missing {name}");
        for registry in [
            src.join("main_unit_tests.rs"),
            src.join("../tests/source_contracts/support.rs"),
        ] {
            assert!(
                std::fs::read_to_string(registry).unwrap().contains(name),
                "unregistered {name}"
            );
        }
    }
    let session = std::fs::read_to_string(src.join("cmd/session.rs")).unwrap();
    assert!(session.contains("last_crash:"));
    let crumbs = std::fs::read_to_string(src.join("crumbs.rs")).unwrap();
    let shipped = crumbs.split("#[cfg(test)]").next().unwrap();
    assert!(!shipped.contains("Mutex"));
    assert!(!shipped.contains("std::fs"));
    assert!(!shipped.contains("unsafe"));
}

#[test]
fn crash_report_preserves_the_boot_pin_and_uses_existing_panic_and_resume_roads() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let session = std::fs::read_to_string(src.join("cmd/session.rs")).unwrap();
    assert!(session.contains("onboarding: stored_onboarding(state.config_root()),\n        last_crash: crash::consume(state.local_data_root()),"));
    let runtime = std::fs::read_to_string(src.join("system_runtime.rs")).unwrap();
    // The panic hook writes the report through `crash::record` and names the
    // kind itself — on one line or, since the origin travels too (t-3014),
    // across rustfmt's several.
    let hook = super::support::block_after(&runtime, "pub(super) fn record_panic(");
    assert!(
        hook.contains("crash::record(")
            && hook.contains("crash::Kind::Panic")
            && hook.contains("crash::Origin {"),
        "the panic hook no longer writes the report through crash::record"
    );
    let main = std::fs::read_to_string(src.join("main.rs")).unwrap();
    assert!(main.contains("hang_watchdog::Monitor::new()"));
    assert!(main.contains("watchdog.resumed()") && main.contains("watchdog.tick(&resume_app)"));
    let watchdog = std::fs::read_to_string(src.join("hang_watchdog.rs")).unwrap();
    assert!(watchdog.contains("run_on_main_thread"));
    assert!(!watchdog.contains("thread::spawn"));
    let crash = std::fs::read_to_string(src.join("crash.rs")).unwrap();
    assert_eq!(crash.matches("pub(crate) struct Limits").count(), 1);
    let environment = super::support::block_after(&crash, "fn environment()");
    for key in ["token", "cookie", "email", "env::vars", "home_dir"] {
        assert!(!environment.contains(key));
    }
    assert!(crash.contains("durable_file::replace_bytes"));
}

#[test]
fn every_plain_sync_tauri_command_has_a_scoped_breadcrumb() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut count = 0;
    for directory in [&src, &src.join("cmd")] {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().is_none_or(|ext| ext != "rs")
                || path
                    .file_name()
                    .is_some_and(|name| name == "main_unit_tests.rs")
            {
                continue;
            }
            let text = std::fs::read_to_string(&path).unwrap();
            let shipped = text.split("#[cfg(test)]").next().unwrap();
            for rest in shipped.split("\n#[tauri::command]\n").skip(1) {
                let signature = rest.split('{').next().unwrap();
                if signature.contains("async fn") {
                    continue;
                }
                let body = rest.split_once('{').unwrap().1;
                assert!(
                    body.trim_start()
                        .starts_with("let _crumb = crate::crumbs::Command::enter("),
                    "unmeasured sync command in {}",
                    path.display()
                );
                count += 1;
            }
        }
    }
    assert!(count > 0);
}

/// t-3014 §2.4 — the crash that becomes a task walks the ledger's one
/// door: `task-create` argv presented from a seated leader, the way the
/// standing-order beat presents `worker-start`. No second ledger API grows
/// beside it, the body is built from the report's named fields alone, the
/// boot site arms the sweep AFTER the sheet's incident is consumed, and the
/// sweep rides the beat that already exists.
#[test]
fn the_crash_task_walks_the_task_create_argv_road_and_no_second_ledger_api_exists() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let read = |name: &str| std::fs::read_to_string(src.join(name)).unwrap();
    let orchestration = read("orchestration.rs");
    let road = super::support::block_after(&orchestration, "pub(crate) fn file_crash_task(");
    for needed in [
        "crash::pending_triage(",
        "file_task_through_seat(host, overrides, &pending.argv, now_ms)",
        "crash::note_triaged(",
    ] {
        assert!(road.contains(needed), "the crash task road lost `{needed}`");
    }
    // The seat road itself (one function, shared with the QA road): the
    // newest seated coordinator this window can sign for, the argv door.
    let seat = super::support::block_after(&orchestration, "pub(crate) fn file_task_through_seat(");
    for needed in ["crate::agent_teams::current_pane_capability(", "run(\n"] {
        assert!(seat.contains(needed), "the seat road lost `{needed}`");
    }
    assert!(
        !road.contains("create_task(") && !road.contains("actor.plan("),
        "the crash task reaches the ledger without the argv road"
    );
    let seat = super::support::block_after(&orchestration, "pub(crate) fn file_task_through_seat(");
    assert!(
        seat.contains("crate::agent_teams::current_pane_capability(") && seat.contains("run(\n"),
        "the shared filing road must present the seat through the argv door"
    );
    assert!(!seat.contains("create_task(") && !seat.contains("actor.plan("));
    let crash = read("crash.rs");
    let argv = super::support::block_after(&crash, "pub(crate) fn task_argv(");
    for word in ["\"task-create\"", "\"--retry-request\"", "\"--spec\""] {
        assert!(argv.contains(word), "the crash task argv lost {word}");
    }
    let body = super::support::block_after(&crash, "fn task_body(");
    for forbidden in [
        "env::var",
        "std::fs",
        "window-errors",
        "log_tail",
        "read_to_string",
        "open_plain_file",
    ] {
        assert!(
            !body.contains(forbidden),
            "the task body reads `{forbidden}` instead of the report's named fields"
        );
    }
    // No second ledger API: the shipped shell never calls core's create_task.
    for (name, text) in super::support::BACKEND_PARTS {
        let shipped = text.split("#[cfg(test)]").next().unwrap();
        assert!(
            !shipped.contains(".create_task("),
            "{name} reaches the ledger without the argv road"
        );
    }
    let session = read("cmd/session.rs");
    let consumed = session
        .find("last_crash: crash::consume(state.local_data_root()),")
        .expect("the boot door");
    let armed = session
        .find("crash_triage::arm(")
        .expect("the boot site no longer arms the crash triage");
    assert!(
        consumed < armed,
        "the sweep is armed before the incident is consumed"
    );
    let triage = read("crash_triage.rs");
    for needed in [
        "orchestration::file_crash_task(",
        "\"crash:triaged\"",
        "tell(app, &ROAD, &said, line.as_deref())",
    ] {
        assert!(triage.contains(needed), "crash_triage.rs lost `{needed}`");
    }
    // The line and the event are said by the three seat roads' one teller.
    let seat_roads = read("seat_triage.rs");
    let tell = super::support::block_after(&seat_roads, "pub(crate) fn tell(");
    for needed in ["crate::note_window_event(", "emit_to(", "road.event"] {
        assert!(tell.contains(needed), "seat_triage::tell lost `{needed}`");
    }
    let tools = read("agent_tools_runtime.rs");
    let beat = super::support::block_after(&tools, "pub(super) fn beat_standing_orders(");
    assert!(
        beat.contains("crash_triage::sweep("),
        "the crash triage no longer rides the standing-order beat"
    );
}

/// The sheet's line is one element, spoken through t() in five catalogs,
/// filled by the `crash:triaged` event, and carries no native title attribute.
#[test]
fn the_crash_sheet_names_the_filed_task_through_the_catalog() {
    let ui = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../ui");
    let html = std::fs::read_to_string(ui.join("index.html")).unwrap();
    let sheet = html
        .split("id=\"crash-sheet\"")
        .nth(1)
        .and_then(|rest| rest.split("</section>").next())
        .expect("the crash sheet");
    assert!(sheet.contains("id=\"crash-filed\""));
    assert!(
        !sheet.contains(" title="),
        "a native title attribute on the crash sheet"
    );
    let shell = std::fs::read_to_string(ui.join("shell.js")).unwrap();
    assert!(shell.contains("listen(\"crash:triaged\""));
    assert!(shell.contains("t(\"crash.filed\""));
    let catalog = std::fs::read_to_string(ui.join("shell-i18n.js")).unwrap();
    assert_eq!(
        catalog.matches("\"crash.filed\":").count(),
        5,
        "crash.filed is not in all five catalogs"
    );
}

#[test]
fn board_ledger_reads_do_not_wait_for_the_authority_on_the_main_thread() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let board = std::fs::read_to_string(src.join("cmd/board.rs")).unwrap();
    let command = super::support::block_after(&board, "pub(crate) fn ledger_agents(");
    assert!(
        command.contains("board_ledger_snapshot()"),
        "ledger_agents still waits for actor.view on the main thread"
    );
    let pane = super::support::block_after(&board, "pub(crate) fn pane_agents(");
    assert!(pane.contains("ledger_states_by_term()"));
    let orchestration = std::fs::read_to_string(src.join("orchestration.rs")).unwrap();
    let states =
        super::support::block_after(&orchestration, "pub(crate) fn ledger_states_by_term()");
    assert!(states.contains("board_ledger_snapshot()"));
    let snapshot =
        super::support::block_after(&orchestration, "pub(crate) fn board_ledger_snapshot()");
    for forbidden in [
        "actor.view",
        "cached_ledger(",
        "with_ledger_seats",
        "WorkflowStore",
        "std::fs",
    ] {
        assert!(!snapshot.contains(forbidden));
    }
    for body in [command, pane] {
        for forbidden in [
            "with_ledger_seats",
            "WorkflowStore",
            "actor.view",
            "cached_ledger(",
        ] {
            assert!(
                !body.contains(forbidden),
                "main-thread ledger read: {forbidden}"
            );
        }
    }
    let runtime = std::fs::read_to_string(src.join("agent_tools_runtime.rs")).unwrap();
    let beat = super::support::block_after(&runtime, "pub(super) fn beat_standing_orders(");
    assert!(beat.contains("StandingBeat::enter()"));
    assert!(beat.find("StandingBeat::enter()").unwrap() < beat.find("spawn_blocking").unwrap());
    assert!(beat.contains("refresh_board_ledger()"));
}

#[test]
fn hang_evidence_names_the_observed_main_scope_and_samples_that_thread() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let watchdog = std::fs::read_to_string(src.join("hang_watchdog.rs")).unwrap();
    assert!(watchdog.contains("crate::crumbs::main_scope()"));
    assert!(watchdog.contains("if answered.is_none()"));
    assert!(watchdog.contains("sample_main_thread()"));
    assert!(watchdog.contains("main_sample_after_detection"));
    let sampler = std::fs::read_to_string(src.join("hang_sample.rs")).unwrap();
    assert!(sampler.contains("Limits::SAMPLE_TIMEOUT_MS"));
    assert!(!sampler.contains("thread::spawn"));
    assert!(!watchdog.contains("Backtrace::"));
    assert!(!watchdog.contains("thread::spawn"));
    let main = std::fs::read_to_string(src.join("main.rs")).unwrap();
    assert!(main.contains("crumbs::MainScope::enter(hang_watchdog::event_name(&event))"));
    assert!(main.contains("crumbs::register_main_thread()"));
}

#[test]
fn graph_overlay_does_not_reenter_the_authority_from_the_main_thread() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let orchestration = std::fs::read_to_string(src.join("orchestration.rs")).unwrap();
    let overlay =
        super::support::block_after(&orchestration, "pub(crate) fn graph_overlay_snapshot()");
    assert!(
        overlay.contains("board_ledger_snapshot()"),
        "board_snapshot still waits for the authority after ledger_agents returns"
    );
    assert!(!overlay.contains("with_ledger_seats("));
}
