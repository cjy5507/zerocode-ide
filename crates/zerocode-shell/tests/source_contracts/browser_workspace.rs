use super::support::block_after;

const BROWSER: &str = include_str!("../../src/cmd/browser.rs");
const CONTROL: &str = include_str!("../../src/cmd/browser_workspace.rs");
const CEF: &str = include_str!("../../src/chromium_browser.rs");
const RUNTIME: &str = include_str!("../../src/agent_tools_runtime.rs");
const SNAPSHOT: &str = include_str!("../../src/cmd/fs.rs");

#[test]
fn human_reads_arbitrary_eval_and_unknown_verbs_are_refused_before_dispatch() {
    let source = include_str!("../../src/browser_workspace.rs");
    assert!(source.contains("pub(crate) fn begin_at("));
    let admission = block_after(source, "fn begin_at(");
    assert!(admission.contains("browser_verb(verb)"));
    assert!(admission.contains("if state.mode != Mode::Agent"));
    assert!(admission.contains("if verb == \"eval\""));
    assert!(!admission.contains("state.owner ="));
}

#[test]
fn every_main_page_mutator_checks_revision_and_keeps_its_operation_until_completion() {
    for name in [
        "browser_navigate",
        "browser_history",
        "browser_reload",
        "browser_stop",
        "browser_devtools",
        "browser_find",
        "browser_find_clear",
        "browser_zoom",
        "browser_scroll",
        "browser_eval",
        "browser_click",
        "browser_type",
        "browser_grab_arm",
        "browser_grab_disarm",
    ] {
        let command = block_after(BROWSER, &format!("fn {name}("));
        assert!(command.contains("expected_revision: Option<u64>"), "{name}");
        let authenticated = command.find("from_the_main_webview(&webview)?").unwrap();
        let admitted = command
            .find("begin_person_control(&app, &state, &label, expected_revision)")
            .unwrap();
        let finished = command
            .find("finish_person_control(operation, result)")
            .unwrap();
        assert!(authenticated < admitted && admitted < finished, "{name}");
    }
}

#[test]
fn handoff_waits_for_old_requests_then_same_pane_fences_before_granting_control() {
    let command = block_after(CONTROL, "async fn set_browser_workspace_mode(");
    assert!(
        command.contains("transition.fence_labels()"),
        "handoff must confirm every affected page before granting control"
    );
    let hidden = command.find("hide_browser_pane(").unwrap();
    let drained = command.find("transition.wait()").unwrap();
    let labels = command.find("transition.fence_labels()").unwrap();
    let fenced = command.find("workspace_page_fence(&pane)").unwrap();
    let confirmed = command.find("transition.confirm_fence()").unwrap();
    let granted = command.find("transition.finish()").unwrap();
    assert!(
        hidden < drained
            && drained < labels
            && labels < fenced
            && fenced < confirmed
            && confirmed < granted
    );
    assert!(command.contains("BROWSER_DOOR_BUDGET + super::browser::BROWSER_CALLBACK_DEADLINE"));
    let fence = block_after(BROWSER, "async fn workspace_page_fence(");
    assert!(fence.contains("page_json(pane, \"1\", BROWSER_CALLBACK_DEADLINE)"));
}

#[test]
fn both_native_engines_create_managed_views_without_an_interactive_visible_frame() {
    let cef_birth = block_after(CEF, "fn begin_create_pane(");
    assert!(cef_birth.contains("window.hidden = i32::from(options.hidden)"));
    let cef_finish = block_after(CEF, "fn finish_create_pane(");
    assert!(
        cef_finish.find("pane.set_hidden(options.hidden)").unwrap()
            < cef_finish.find("pane.set_bounds(").unwrap()
    );
    let wry = block_after(BROWSER, "fn open_webkit_browser_pane(");
    assert!(wry.contains("builder.focused(false)"));
    assert!(wry.contains("if managed { 0.0 } else { width.max(1.0) }"));
    assert!(wry.contains("if managed { 0.0 } else { height.max(1.0) }"));
    assert!(
        wry.find("place_native_browser(").unwrap()
            < wry.find("initialize_workspace_zoom(").unwrap()
    );
}

#[test]
fn visibility_decision_and_bounds_run_together_on_the_native_ui_thread() {
    assert!(
        BROWSER.contains("fn place_native_browser("),
        "native visibility must share the state check and the bounds operation"
    );
    let wry = block_after(BROWSER, "fn place_native_browser(");
    assert!(wry.contains("pane.with_webview(move |platform|"));
    assert!(wry.contains(".with_placement(&label, revision, |allowed|"));
    assert!(wry.find("setHidden: true").unwrap() < wry.find(".with_placement(").unwrap());
    assert!(wry.find("SetIsVisible(false)").unwrap() < wry.find(".with_placement(").unwrap());
    assert!(wry.contains("SetBounds("));
    assert!(wry.contains("setFrame: frame"));
    let cef = block_after(CEF, "fn place(");
    assert!(cef.contains("on_ui(&self.app, &self.label"));
    assert!(cef.contains(".with_placement(&label, revision, |allowed|"));
    assert!(cef.find("pane.set_hidden(true)").unwrap() < cef.find(".with_placement(").unwrap());
    assert!(cef.contains("pane.set_bounds("));
}

#[test]
fn managed_popups_use_one_deferred_backend_route_with_the_captured_revision() {
    assert_eq!(
        CEF.matches("crate::cmd::browser::workspace_popup(").count(),
        2
    );
    assert!(BROWSER.contains("workspace_popup(&popup_app, &popup_label, url.as_str())"));
    let route = block_after(BROWSER, "fn workspace_popup(");
    assert!(route.contains("snapshot.mode == crate::browser_workspace::Mode::Human"));
    assert!(route.contains("revision: Some(snapshot.revision)"));
    assert!(route.contains("snapshot.mode == crate::browser_workspace::Mode::Agent"));
    let deferred = route.find("tauri::async_runtime::spawn(").unwrap();
    let main = route.find("app.run_on_main_thread(").unwrap();
    let checked = route.find(".with_agent_popup(").unwrap();
    assert!(deferred < main && main < checked);
    assert!(!route.contains("set_focus(") && !route.contains("open_browser_pane("));
}

#[test]
fn initial_zoom_uses_the_existing_setting_before_the_native_label_is_published() {
    for name in ["open_webkit_browser_pane", "open_chromium_browser_pane"] {
        let birth = block_after(BROWSER, &format!("fn {name}("));
        assert!(
            birth.contains(".browser.default_zoom_level") || birth.contains(".default_zoom_level")
        );
        let applied = birth
            .find("initialize_workspace_zoom(&pane, zoom_level_to_scale(level))")
            .unwrap();
        let returned = birth.rfind("Ok(label)").unwrap();
        assert!(applied < returned, "{name}");
        assert!(!birth.contains("browser_panes().insert("), "{name}");
    }
    let publish = block_after(BROWSER, "fn open_browser_pane(");
    assert!(
        publish.find("open_chromium_browser_pane(").unwrap()
            < publish.find(".publish(&label, revision,").unwrap()
    );
    assert!(
        publish.find("open_webkit_browser_pane(").unwrap()
            < publish.find(".publish(&label, revision,").unwrap()
    );
    let initial = block_after(BROWSER, "fn initialize_workspace_zoom(");
    assert!(initial.contains("pane.with_webview(move |platform|"));
    assert!(initial.contains("setPageZoom: scale"));
    assert!(initial.contains("controller.SetZoomFactor(scale)"));
    assert!(initial.contains("controller.ZoomFactor(&mut actual)"));
    assert!(initial.contains("workspace_zoom_unconfirmed"));
    assert!(!BROWSER.contains("initial: Option<bool>"));
}

#[test]
fn timed_out_open_is_cancelled_and_managed_viewport_does_not_claim_unconfirmed_success() {
    let dispatch = block_after(RUNTIME, "async fn dispatch_browser_command(");
    assert!(
        dispatch.contains("cmd::browser::cancel_workspace_open(")
            && dispatch.contains("creation.revision")
    );
    assert!(dispatch.contains("workspace_open_timeout"));
    let guarded = block_after(RUNTIME, "async fn answer_browser_command(");
    let refusal = guarded.find("workspace_viewport_unconfirmed").unwrap();
    assert!(refusal < guarded.find("dispatch_browser_command(").unwrap());
    assert!(guarded.contains("begin_actor_target("));
    assert!(guarded.contains("let actor = step_actor(app, pane)"));
    let loop_body = block_after(RUNTIME, "async fn browser_loop(");
    assert!(loop_body.contains("request.workspace_revision"));
    assert!(!loop_body.contains("browser_revision("));
    assert!(loop_body.contains("reply.closed()"));
}

#[test]
fn capture_limits_survive_caller_cancellation_until_the_native_callback_completes() {
    assert!(
        SNAPSHOT.contains("async fn browser_snapshot_png_for("),
        "native capture must keep the shared lease through its callback"
    );
    let capture = block_after(SNAPSHOT, "async fn browser_snapshot_png_for(");
    assert!(
        capture
            .find(".begin_snapshot_for(label, spectator)")
            .unwrap()
            < capture
                .find("snapshot_leased(pane, frame, Default::default())")
                .unwrap()
    );
    let leased = block_after(SNAPSHOT, "async fn snapshot_leased(");
    assert!(leased.contains("let _frame = frame"));
    assert!(
        leased.contains("snapshot_webview_png_held(pane, None, Some((Box::new(frame), check)))")
    );
    let webkit = include_str!("../../src/artifact_webkit.rs");
    let callback = block_after(webkit, "fn take_snapshot_png_held(");
    assert!(callback.contains("let _held = held.borrow_mut().take()"));
    let ipc = block_after(SNAPSHOT, "async fn browser_snapshot(");
    assert!(ipc.contains("workspace_snapshot_unsupported"));
    assert!(ipc.contains("AGENT_SCREENSHOT_MAX_BYTES"));
}

#[test]
fn agent_close_requires_the_origin_epoch_and_only_confirmed_native_close_can_be_acknowledged() {
    let ipc = block_after(BROWSER, "fn close_browser_pane(");
    assert!(ipc.contains("expected_revision: Option<u64>"));
    assert!(ipc.contains("from_the_main_webview(&webview)?"));
    assert!(ipc.contains("close_native_browser("));
    let closing = block_after(BROWSER, "fn close_native_browser(");
    assert!(closing.contains(".closed_at(label, revision)"));
    assert!(closing.contains("expected_revision.is_some_and(|expected| expected != revision)"));
    assert!(
        closing.find("pane.close()").unwrap()
            < closing.find(".confirm_closed(label, revision)").unwrap()
    );
    let canceled = block_after(BROWSER, "fn cancel_workspace_open(");
    assert!(canceled.contains(".cancel_unpublished(label, revision,"));
    assert!(canceled.contains("close_native_browser(app, &main, state, label, Some(revision))"));
    assert!(canceled.contains("revision: Some(revision)"));
    let pending = block_after(RUNTIME, "impl Drop for PendingBrowserOpen");
    assert!(pending.contains("cancel_workspace_open("));
    assert!(pending.contains("self.revision"));
}

#[test]
fn ownership_adapters_use_held_terminal_identity_and_do_not_select_a_model() {
    assert!(
        CONTROL.contains("fn owner_actor("),
        "ownership must resolve an actual held terminal"
    );
    let owner = block_after(CONTROL, "fn owner_actor(");
    assert!(owner.contains("let term: u32"));
    assert!(owner.contains("crate::artifact_runtime::held_pane(app, &hooks::pane_key_of(term))"));
    assert!(
        !owner.contains("facts.agent") && !owner.contains(".model") && !owner.contains(".provider")
    );
    let admission = block_after(RUNTIME, "async fn answer_browser_command(");
    assert!(admission.contains("let actor = step_actor(app, pane)"));
    assert!(!admission.contains(".model") && !admission.contains(".provider"));
    let artifacts = include_str!("../../src/artifact_runtime.rs");
    let held = block_after(artifacts, "fn held_pane(");
    assert!(held.contains("facts_of_pane(app, term)?"));
    let facts = block_after(artifacts, "fn facts_of_pane(");
    assert!(facts.contains("state.terminals().contains_key(&term)"));
    assert!(facts.contains("Some(pane_facts(heard, seat))"));
}

#[test]
fn original_revision_admission_and_observation_completion_share_backend_state() {
    let source = include_str!("../../src/browser_workspace.rs");
    let admission = block_after(source, "fn begin_at(");
    assert!(admission.contains("workspace_revision_required:"));
    assert!(
        source.contains("fn finish_observation("),
        "a read needs a checked completion"
    );
    let completion = block_after(source, "fn finish_observation(");
    for guard in [
        "state.closed",
        "state.revision",
        "state.mode",
        "state.owner",
    ] {
        assert!(completion.contains(guard), "completion must check {guard}");
    }
    let answer = block_after(RUNTIME, "async fn answer_browser_command(");
    assert!(answer.contains("parse_workspace_command("));
    assert!(answer.contains("finish_observation("));
    assert!(answer.contains("stamp_browser_observation("));
    assert!(!RUNTIME.contains("fn browser_revision("));
}

#[test]
fn wire_validates_revision_before_the_request_enters_the_browser_queue() {
    let hookd = include_str!("../../../zerocode-hookd/src/lib.rs");
    let receiver = block_after(hookd, "async fn receive_browser_command(");
    assert!(receiver.contains("workspace_revision_of("));
    assert!(
        receiver.find("workspace_revision_of(").unwrap()
            < receiver.find("state.browser.send(").unwrap()
    );
    assert!(hookd.contains("pub workspace_revision: Option<u64>"));
}

#[test]
fn nested_walk_recipe_and_code_entry_keep_their_own_observation_revision() {
    let walk = include_str!("../../src/computer_use/errand/walk.rs");
    let recipe = include_str!("../../src/computer_use/recipe_run.rs");
    let enter = include_str!("../../src/computer_use/enter.rs");
    assert!(walk.contains("workspace_revision"));
    assert!(recipe.contains("observation_revision("));
    assert!(enter.contains("into.workspace_revision()"));
    let typer = block_after(RUNTIME, "impl computer_use::enter::Typer for LiveTyper");
    assert!(typer.contains("workspace_revision: Option<u64>"));
    assert!(!typer.contains(".snapshot(") && !typer.contains("browser_revision("));
}

fn arm<'a>(dispatch: &'a str, opens: &str, next: &str) -> &'a str {
    let start = dispatch.find(opens).unwrap_or_else(|| panic!("{opens}"));
    let end = dispatch[start..]
        .find(next)
        .map_or(dispatch.len(), |at| start + at);
    &dispatch[start..end]
}

#[test]
fn step_frames_are_bound_to_the_steps_control_and_rechecked_before_they_are_kept() {
    let evidence = include_str!("../../src/evidence_runtime.rs");
    let leave = block_after(evidence, "fn leave_browser_evidence(");
    assert!(leave.contains("control: Option<crate::browser_workspace::StepControl>"));
    assert!(leave.contains("Frame::After(Capture::Browser {"));
    let take = block_after(evidence, "impl Surface for Capture {");
    let browser = &take[take.find("async fn take(self)").unwrap()..];
    assert!(browser.contains("browser_evidence_png(&app, &state, &label, control.as_ref())"));
    assert!(browser.contains("Ok(None) => Err(FrameSkipped::ControlChanged)"));
    assert!(!take.contains("browser_snapshot_png("));
    let capture = block_after(SNAPSHOT, "async fn browser_evidence_png(");
    let admitted = capture.find(".admit_evidence(label, control)").unwrap();
    let taken = capture
        .find("snapshot_leased(pane, frame, check.clone())")
        .unwrap();
    let kept = capture.find("check.holds()").unwrap();
    assert!(admitted < taken && taken < kept);
    for minted in [
        "finish_observation",
        "begin_at(",
        "needs_observation",
        "stamp_browser_observation",
    ] {
        assert!(!capture.contains(minted), "{minted}");
    }
    let source = include_str!("../../src/browser_workspace.rs");
    let admission = block_after(source, "fn admit_evidence(");
    assert!(
        admission.find("agent_holds(&state, control)").unwrap()
            < admission
                .rfind(".begin_snapshot_for(label, false)")
                .unwrap()
    );
    assert!(!admission.contains("needs_observation") && !admission.contains("in_flight"));
    let held = block_after(source, "fn agent_holds(");
    for guard in [
        "state.closed",
        "Mode::Agent",
        "control.revision",
        "control.owner",
    ] {
        assert!(held.contains(guard), "{guard}");
    }
    let step = block_after(RUNTIME, "async fn browser_step_at(");
    assert!(step.contains(
        "answer_browser_command(app, argv, pane, expected_revision, &mut control).await"
    ));
    assert!(step.contains("control,"));
    let answer = block_after(RUNTIME, "async fn answer_browser_command(");
    assert!(answer.contains(".and_then(crate::browser_workspace::Operation::step_control)"));
}

#[test]
fn only_a_refusal_before_dispatch_is_known_to_have_no_effect() {
    let answer = block_after(RUNTIME, "async fn answer_browser_command(");
    assert!(answer.contains("let mut dispatched = crate::browser_workspace::Dispatch::Uncertain;"));
    assert!(
        answer.contains(
            "dispatch_browser_command(app, argv, pane, expected_revision, &mut dispatched)"
        )
    );
    assert_eq!(answer.matches(".settle(dispatched)").count(), 2);
    for inferred in [
        "PAGE_TIMED_OUT",
        "PAGE_GONE",
        "PAGE_ANSWER_UNREADABLE",
        "PAGE_SEND_FAILED",
        "stderr.contains",
        "stderr.starts_with",
    ] {
        assert!(!answer.contains(inferred), "{inferred}");
    }
    let unsent = block_after(RUNTIME, "fn refused_unsent(");
    assert!(unsent.contains("*dispatched = crate::browser_workspace::Dispatch::NotSent;"));
    assert!(unsent.contains("browser_refused(stderr)"));
    let dispatch = block_after(RUNTIME, "async fn dispatch_browser_command(");
    for (opens, next, sent) in [
        ("(\"open\", 2) =>", "(\"close\", 2) =>", "app.emit_to("),
        ("(\"close\", 2) =>", "(\"viewport\", 3) =>", "app.emit_to("),
        ("(\"viewport\", 3) =>", "(\"scroll\", 3) =>", "app.emit_to("),
        (
            "(\"scroll\", 3) =>",
            "(\"diagnose\", _) =>",
            "automate_scroll(",
        ),
        (
            "(\"goto\", 3) =>",
            "(\"eval\", 3) | (\"eval\", 4) =>",
            "pane.navigate(url)",
        ),
        (
            "(\"eval\", 3) | (\"eval\", 4) =>",
            "(\"read\", 2) | (\"read\", 3) =>",
            "automate_eval(",
        ),
        (
            "(\"fill\", _) =>",
            "(\"screenshot\", _) =>",
            "automate_fill(",
        ),
    ] {
        let arm = arm(dispatch, opens, next);
        let sent = arm.find(sent).unwrap_or_else(|| panic!("{opens}"));
        assert!(arm[..sent].contains("refused_unsent("), "{opens}");
        assert!(!arm[sent..].contains("refused_unsent("), "{opens}");
    }
    let click = arm(
        dispatch,
        "(\"click\", 3) | (\"click\", 4) | (\"click\", 5) =>",
        "(\"type\", 4) =>",
    );
    assert_eq!(click.matches("refused_unsent(").count(), 1);
    assert!(click.find("refused_unsent(").unwrap() < click.find("match pressed {").unwrap());
    let later = click.find("match later.await {").unwrap();
    let later = &click[later..later + click[later..].find("};").unwrap()];
    assert!(!later.contains("refused_unsent("));
    for (opens, next) in [
        ("(\"find\", 3) =>", "(\"goto\", 3) =>"),
        ("(\"type\", 4) =>", "(\"wait\", 3) | (\"wait\", 4) =>"),
    ] {
        assert!(
            !arm(dispatch, opens, next).contains("refused_unsent("),
            "{opens}"
        );
    }
    let fill = arm(dispatch, "(\"fill\", _) =>", "(\"screenshot\", _) =>");
    assert!(fill.contains("page_refused(&label, words)"));
    assert!(fill.contains("return browser_refused(format!(\"zerocode-browser: {words}\"));"));
    let usage = &dispatch[dispatch.rfind("_ => ").unwrap()..];
    assert!(usage.starts_with("_ => refused_unsent("));
    assert!(usage.contains("zerocode_core::agent_browser::usage()"));
}

#[test]
fn windows_placement_moves_sizes_and_shows_the_wry_container_without_activation() {
    let wry = block_after(BROWSER, "fn place_native_browser(");
    let decided = wry.find(".with_placement(").unwrap();
    let parent = wry.find(".ParentWindow(&mut container)").unwrap();
    let hidden = wry
        .find("set_native_container_shown(container, false)")
        .unwrap();
    assert!(parent < hidden && hidden < decided);
    let placed = &wry[decided..];
    let moved = placed.find("SetWindowPos(").unwrap();
    let sized = placed.find("right: width,").unwrap();
    let shown = placed
        .find("set_native_container_shown(container, true)")
        .unwrap();
    let visible = placed.find(".SetIsVisible(true)").unwrap();
    assert!(moved < sized && sized < shown && shown < visible);
    assert!(placed.contains("left: 0,") && placed.contains("top: 0,"));
    assert!(placed.contains("SWP_NOACTIVATE") && placed.contains("SWP_NOZORDER"));
    for activation in [
        "ShowWindow(",
        "SW_SHOW",
        "SetForegroundWindow",
        "SetActiveWindow",
        "BringWindowToTop",
    ] {
        assert!(!wry.contains(activation), "{activation}");
    }
    assert!(wry.contains("native_container_holds_focus(container)"));
    let visibility = block_after(BROWSER, "fn set_native_container_shown(");
    for flag in [
        "SWP_SHOWWINDOW",
        "SWP_HIDEWINDOW",
        "SWP_NOACTIVATE",
        "SWP_NOMOVE",
        "SWP_NOSIZE",
        "SWP_NOZORDER",
    ] {
        assert!(visibility.contains(flag), "{flag}");
    }
    let focus = block_after(BROWSER, "fn native_container_holds_focus(");
    assert!(focus.contains("GetFocus()") && focus.contains("IsChild(container, focus)"));
    let hide = block_after(BROWSER, "fn hide_browser_pane(");
    let windows = &hide[hide.find("#[cfg(target_os = \"windows\")]").unwrap()..];
    let windows = &windows[..windows.find("#[cfg(not(any(").unwrap()];
    assert!(windows.contains("controller.ParentWindow(&mut container)"));
    assert!(windows.contains("native_container_holds_focus(container)"));
    assert!(windows.contains(".SetIsVisible(false)"));
    assert!(windows.contains("set_native_container_shown(container, false)"));
    assert!(!windows.contains("pane.hide()"));
}

#[test]
fn handoff_waits_for_a_known_navigation_before_the_page_fence() {
    let command = block_after(CONTROL, "async fn set_browser_workspace_mode(");
    let labels = command.find("transition.fence_labels()").unwrap();
    let settled = command
        .find("settle_known_navigation(&state, &labels)")
        .unwrap();
    let fenced = command.find("workspace_page_fence(&pane)").unwrap();
    let confirmed = command.find("transition.confirm_fence()").unwrap();
    assert!(labels < settled && settled < fenced && fenced < confirmed);
    let settle = block_after(BROWSER, "async fn settle_known_navigation(");
    assert!(settle.contains(".nav.pending(now)"));
    assert!(settle.contains("workspace_paused:"));
    let note = block_after(BROWSER, "fn note_navigation_request(");
    assert!(note.contains(".nav.requested("));
    let navigate = block_after(BROWSER, "fn browser_navigate(");
    assert!(
        navigate
            .find("note_navigation_request(&state, &label)")
            .unwrap()
            < navigate.find("pane.navigate(parsed)").unwrap()
    );
    let dispatch = block_after(RUNTIME, "async fn dispatch_browser_command(");
    let goto = arm(
        dispatch,
        "(\"goto\", 3) =>",
        "(\"eval\", 3) | (\"eval\", 4) =>",
    );
    assert!(
        goto.find("cmd::browser::note_navigation_request(&state, &argv[1])")
            .unwrap()
            < goto.find("pane.navigate(url)").unwrap()
    );
    let source = include_str!("../../src/browser_workspace.rs");
    let admission = block_after(source, "fn begin_at(");
    assert!(admission.contains("reads_history(verb)"));
    let placement = block_after(source, "fn with_placement<T>(");
    assert!(!placement.contains("in_flight"));
}
