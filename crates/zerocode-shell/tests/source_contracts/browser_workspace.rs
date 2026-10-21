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
    assert!(wry.contains(".with_placement(&label, revision, watching, |placement|"));
    assert!(wry.find("setHidden: true").unwrap() < wry.find(".with_placement(").unwrap());
    assert!(wry.find("SetIsVisible(false)").unwrap() < wry.find(".with_placement(").unwrap());
    assert!(wry.contains("SetBounds("));
    assert!(wry.contains("setFrame: frame"));
    // No shield stands over a WebKit or WebView2 pane (t-66513): it is shown
    // for the person's hand alone, and a watch is refused to the snapshots.
    assert!(
        wry.contains("let allowed = placement == crate::browser_workspace::Placement::Control;")
    );
    assert!(wry.contains("return Err(WATCH_UNSUPPORTED.to_string());"));
    let cef = block_after(CEF, "fn place(");
    assert!(cef.contains("on_ui(&self.app, &self.label"));
    assert!(cef.contains(".with_placement(&label, revision, watching, |placement|"));
    assert!(cef.find("pane.set_hidden(true)").unwrap() < cef.find(".with_placement(").unwrap());
    // The decision and the pane's frame, shield and showing are one native
    // operation (t-66513): the moves are staged inside the decision.
    assert!(
        cef.find(".with_placement(").unwrap()
            < cef
                .find("crate::browser_workspace::stage_pane(&mut stage, placement, bounds, shown)")
                .unwrap()
    );
    let staged = block_after(
        CEF,
        "impl crate::browser_workspace::PaneStage for Staged<'_> {",
    );
    assert!(staged.contains(".set_bounds(bounds[0], bounds[1], bounds[2], bounds[3])"));
    assert!(staged.contains("self.pane.shield.raise(view, dress)"));
    assert!(staged.contains("self.pane.shield.lower();"));
    assert!(staged.contains("self.pane.set_hidden(false)"));
    let source = include_str!("../../src/browser_workspace.rs");
    let stage = block_after(source, "pub(crate) fn stage_pane(");
    let bounds = stage.find("stage.bounds(bounds)?").unwrap();
    let shield = stage
        .find("stage.shield(shown && placement == Placement::Watch)?")
        .unwrap();
    let shown = stage.find("stage.show()?").unwrap();
    assert!(bounds < shield && shield < shown);
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
        capture.find(".lease_frame(label, spectator)").unwrap()
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
    // A change that names no revision is judged at the one its own agent's
    // last look issued (t-67064): no refusal for a missing number is left.
    assert!(admission.contains("own_look(&state, actor, label)?"));
    assert!(!source.contains(concat!("workspace_", "revision_required")));
    let own = block_after(source, "fn own_look(");
    assert!(own.contains("Some((seer, revision)) if seer == actor"));
    assert!(
        source.contains("fn finish_observation("),
        "a read needs a checked completion"
    );
    // A look that lifts a pause completes under the person's stop's own lock
    // (t-66200); every look completes through the one checked body.
    let finishing = block_after(source, "fn finish_observation(");
    assert!(
        finishing.contains("unless_persons_stop(|| self.observed())")
            && finishing.contains("self.observed()")
    );
    let completion = block_after(source, "fn observed(");
    assert!(completion.contains("self.refused(&state, lifts)"));
    assert!(completion.contains("state.seen = self.owner.clone()"));
    // The one reading a look completes by, and is answered early by when
    // its tab leaves its agent's hands (t-67064).
    let refused = block_after(source, "fn refused(");
    for guard in [
        "state.closed",
        "state.revision",
        "state.mode",
        "state.owner",
    ] {
        assert!(refused.contains(guard), "completion must check {guard}");
    }
    assert!(
        block_after(source, "async fn taken(").contains("self.refused(&state, self.lifts(&state))")
    );
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
            "cmd::browser::navigate_known(",
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
    // A form other than the one read is the executor's own refusal, unsent
    // (t-66200): the arm infers nothing from the report.
    assert!(!fill.contains("report.stale"));
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
        .find("settle_known_navigation(&app, &state, &labels)")
        .unwrap();
    let fenced = command.find("workspace_page_fence(&pane)").unwrap();
    let confirmed = command.find("transition.confirm_fence()").unwrap();
    assert!(labels < settled && settled < fenced && fenced < confirmed);
    let settle = block_after(BROWSER, "async fn settle_known_navigation(");
    assert!(settle.contains("probe_known_navigation(app, state, label)"));
    assert!(settle.contains("workspace_paused:"));
    let note = block_after(BROWSER, "fn note_navigation_request(");
    assert!(note.contains(".nav.admit("));
    let known = block_after(BROWSER, "pub(crate) fn navigate_known(");
    assert!(
        known.find("note_navigation_request(").unwrap()
            < known.find("send_known_navigation(").unwrap()
    );
    let navigate = block_after(BROWSER, "fn browser_navigate(");
    assert!(
        navigate
            .find("navigate_known(&app, &pane, &label, parsed")
            .unwrap()
            < navigate.find(".taken()").unwrap()
    );
    let dispatch = block_after(RUNTIME, "async fn dispatch_browser_command(");
    let goto = arm(
        dispatch,
        "(\"goto\", 3) =>",
        "(\"eval\", 3) | (\"eval\", 4) =>",
    );
    let checked = goto.find(".navigation_check(&argv[1], pane)").unwrap();
    let sent = goto
        .find("cmd::browser::navigate_known(app, &pane, &argv[1], url, check)")
        .unwrap();
    assert!(checked < sent && sent < goto.find("sent.taken().await").unwrap());
    let source = include_str!("../../src/browser_workspace.rs");
    let admission = block_after(source, "fn begin_at(");
    assert!(admission.contains("reads_history(verb)"));
    let placement = block_after(source, "fn with_placement<T>(");
    assert!(!placement.contains("in_flight"));
}

#[test]
fn a_capture_starts_only_inside_its_steps_control_and_a_transfer_waits_for_the_send() {
    let source = include_str!("../../src/browser_workspace.rs");
    let start = block_after(source, "pub(crate) fn start<T>(");
    let held = start.find("agent_holds(&state, control)").unwrap();
    let counted = start.find("state.capturing += 1").unwrap();
    let guarded = start.find("Sending(workspace)").unwrap();
    let sent = start.rfind("send()").unwrap();
    assert!(held < counted && counted < guarded && guarded < sent);
    let sending = block_after(source, "impl Drop for Sending");
    assert!(sending.contains("state.capturing = state.capturing.saturating_sub(1)"));
    assert!(sending.contains("drained.notify_waiters()"));
    let wait = block_after(source, "pub(crate) async fn wait(&self)");
    assert!(wait.contains("state.capturing == 0"));
    let finish = block_after(
        source,
        "pub(crate) fn finish(mut self) -> Result<Snapshot, String>",
    );
    assert!(finish.contains("state.capturing != 0"));
    let webkit = block_after(SNAPSHOT, "async fn snapshot_webview_png_held(");
    assert!(webkit.find("check.start(").unwrap() < webkit.find("take_snapshot_png_held(").unwrap());
    let cef = block_after(CEF, "pub(crate) fn snapshot_png<F>(");
    assert!(cef.find("check.start(").unwrap() < cef.find("\"Page.captureScreenshot\"").unwrap());
    let evidence = block_after(SNAPSHOT, "async fn browser_evidence_png(");
    assert!(
        evidence.find("if !check.holds()").unwrap() < evidence.find("bytes.map(Some)").unwrap()
    );
}

#[test]
fn only_the_engines_answer_and_idle_reading_settle_a_known_navigation() {
    let nav = include_str!("../../src/browser_nav_state.rs");
    let pending = block_after(nav, "pub(super) fn pending(&self) -> bool {");
    for clock in ["Instant", "Duration", "now", "observed(", "state"] {
        assert!(!pending.contains(clock), "{clock}");
    }
    for event in [
        "pub(super) fn started(",
        "pub(super) fn finished(",
        "pub(super) fn failed(",
        "pub(super) fn observed(",
        "pub(super) fn dead_if_still(",
    ] {
        assert!(!block_after(nav, event).contains("request"), "{event}");
    }
    assert!(nav.contains("static NEXT_REQUEST: AtomicU64"));
    assert!(block_after(nav, "pub(super) fn admit(").contains("stage: Stage::Admitted"));
    let idle = block_after(nav, "pub(super) fn settle_if_idle(");
    assert!(idle.contains("!loading") && idle.contains("stage: Stage::Answered(_)"));
    let settle = block_after(BROWSER, "async fn settle_known_navigation(");
    for release in [
        ".nav.admit(",
        ".nav.started(",
        ".nav.finished(",
        ".nav.failed(",
        "dead_if_still(",
    ] {
        assert!(!settle.contains(release), "{release}");
    }
    let page_load = block_after(BROWSER, ".on_page_load(");
    let cef_events = block_after(CEF, "fn emit_nav(");
    let cef_failure = block_after(CEF, "fn on_load_error(");
    for event in [page_load, cef_events, cef_failure] {
        for answer in [
            "navigation_answered(",
            "navigation_settled(",
            "navigation_withdrawn(",
        ] {
            assert!(!event.contains(answer), "{answer}");
        }
    }
    let probe = block_after(BROWSER, "async fn probe_known_navigation(");
    assert!(!probe.contains("readyState") && !probe.contains("page_json("));
}

#[test]
fn a_known_navigation_is_sent_inside_its_steps_control_and_answered_by_its_engine() {
    let cef_send = block_after(CEF, "fn send_navigation(");
    let checked = cef_send.find("check.start(").unwrap();
    let admitted = cef_send
        .find("navigation_sent(&state, &label, number)")
        .unwrap();
    let sent = cef_send
        .find("host.send_dev_tools_message(Some(&message))")
        .unwrap();
    assert!(checked < admitted && admitted < sent);
    assert!(cef_send.contains("\"Page.navigate\""));
    assert!(cef_send.contains("navigation_withdrawn(&state, &label, number)"));
    let cef_idle = block_after(CEF, "pub(crate) fn settle_navigation(");
    assert!(cef_idle.contains("pane.browser.is_loading() == 1"));
    assert!(cef_idle.contains("navigation_settled("));
    let observer = block_after(CEF, "fn on_dev_tools_method_result(");
    assert!(observer.contains("self.navigations.borrow_mut().remove(&message_id)"));
    let native = include_str!("../../src/browser_native_nav.rs");
    for (engine, sends) in [
        (
            "#[cfg(all(target_os = \"macos\", not(feature = \"chromium-browser\")))]\nfn send_known_navigation(",
            "crate::browser_native_nav::webkit::send(",
        ),
        (
            "#[cfg(target_os = \"windows\")]\nfn send_known_navigation(",
            "crate::browser_native_nav::webview2::send(",
        ),
    ] {
        let send = block_after(BROWSER, engine);
        let checked = send.find("check.start(").unwrap();
        let admitted = send
            .find("navigation_sent(&state, &native_label, number)")
            .unwrap();
        let sent = send.find(sends).unwrap();
        assert!(checked < admitted && admitted < sent, "{engine}");
        assert!(send.contains("navigation_withdrawn(&state, &native_label, number)"));
    }
    let webkit = block_after(native, "pub(crate) unsafe fn send(");
    assert!(webkit.find("loadRequest(request)").unwrap() < webkit.find("isLoading()").unwrap());
    let watch = block_after(native, "pub(crate) fn watch(");
    for native_event in [
        "add_NavigationStarting(",
        "add_NavigationCompleted(",
        "NavigationId(&mut navigation)",
        "COREWEBVIEW2_WEB_ERROR_STATUS_OPERATION_CANCELED",
    ] {
        assert!(watch.contains(native_event), "{native_event}");
    }
    assert!(native.contains("\"Page.navigate\""));
    let opening = block_after(BROWSER, "fn open_webkit_browser_pane(");
    assert!(opening.contains("|| managed"));
    assert!(
        opening
            .find("watch_native_loads(app, &pane, &label)")
            .unwrap()
            < opening
                .find("navigate_known(app, &pane, &label, parsed")
                .unwrap()
    );
    let cef_events = block_after(CEF, "fn emit_nav(");
    assert!(cef_events.contains("arm_dead_display(&self.app, &self.label, generation)"));
}

#[test]
fn an_executors_refusal_before_its_first_send_keeps_control_and_one_after_it_does_not() {
    let sending = block_after(BROWSER, "pub(crate) async fn send_after_checks<");
    let checked = sending
        .find("checked.map_err(BrowserRefusal::Unsent)?")
        .unwrap();
    let minted = sending
        .find("pane().map_err(BrowserRefusal::Unsent)?")
        .unwrap();
    let sent = sending
        .find("send(checked, pane).await.map_err(Into::into)")
        .unwrap();
    assert!(checked < minted && minted < sent);
    // A send's failure said only in words is `Sent`: only a refusal that
    // names itself `Unsent` keeps the tab in its agent's hands (t-66200).
    let words = block_after(BROWSER, "impl From<String> for BrowserRefusal {");
    assert!(words.contains("Self::Sent(why)") && !words.contains("Unsent"));
    let form = include_str!("../../src/cmd/browser/form.rs");
    for (source, executor) in [
        (BROWSER, "async fn automate_click("),
        (BROWSER, "async fn automate_type("),
        (BROWSER, "async fn automate_scroll("),
        (BROWSER, "async fn automate_eval("),
        (BROWSER, "async fn automate_find("),
        (BROWSER, "async fn press_mark("),
        (form, "async fn automate_fill("),
    ] {
        let body = block_after(source, executor);
        let routed = body
            .find("send_after_checks(")
            .unwrap_or_else(|| panic!("{executor}"));
        assert!(
            body.contains("|| browser_pane_of(app, state, label),"),
            "{executor}"
        );
        for early in [
            "page_json(",
            "press(",
            "browser_pane_of(app, state, label)?",
        ] {
            assert!(!body[..routed].contains(early), "{executor}: {early}");
        }
    }
    let refused = block_after(RUNTIME, "fn executor_refused(");
    let unsent = refused.find("BrowserRefusal::Unsent(why)").unwrap();
    let not_sent = refused.find("refused_unsent(dispatched,").unwrap();
    let after = refused.find("BrowserRefusal::Sent(why)").unwrap();
    let uncertain = refused.find("browser_refused(").unwrap();
    assert!(unsent < not_sent && not_sent < after && after < uncertain);
    for inferred in ["contains(", "starts_with(", "PAGE_", "Dispatch::NotSent"] {
        assert!(!refused.contains(inferred), "{inferred}");
    }
    let dispatch = block_after(RUNTIME, "async fn dispatch_browser_command(");
    for (opens, next) in [
        ("(\"scroll\", 3) =>", "(\"diagnose\", _) =>"),
        (
            "(\"eval\", 3) | (\"eval\", 4) =>",
            "(\"read\", 2) | (\"read\", 3) =>",
        ),
        (
            "(\"click\", 3) | (\"click\", 4) | (\"click\", 5) =>",
            "(\"type\", 4) =>",
        ),
        ("(\"fill\", _) =>", "(\"screenshot\", _) =>"),
    ] {
        let arm = arm(dispatch, opens, next);
        assert!(
            arm.contains("executor_refused(dispatched, refusal)"),
            "{opens}"
        );
        assert!(!arm.contains("Err(why) => browser_refused("), "{opens}");
    }
    for (opens, next, answer) in [
        ("(\"find\", 3) =>", "(\"goto\", 3) =>", "found_answer("),
        ("(\"type\", 4) =>", "(\"type\", 5) if", "typed_answer("),
        (
            "(\"type\", 5) if",
            "(\"wait\", 3) | (\"wait\", 4) =>",
            "typed_answer(",
        ),
    ] {
        let arm = arm(dispatch, opens, next);
        assert!(
            arm.contains(answer) && arm.contains("dispatched,"),
            "{opens}"
        );
    }
    for answer in ["fn typed_answer(", "fn found_answer("] {
        assert!(
            block_after(RUNTIME, answer)
                .contains("Err(refusal) => executor_refused(dispatched, refusal)"),
            "{answer}"
        );
    }
    let found = block_after(RUNTIME, "fn found_answer(");
    let whole = found.find("Ok(Some(found)) =>").unwrap();
    let missing = found.find("Ok(None) => browser_refused(").unwrap();
    assert!(whole < missing);
    assert!(!found.contains("\"count\": 0") && !found.contains("unwrap_or"));
}

/// An input script's body, from its opener to the raw string's end.
fn raw_body<'a>(source: &'a str, opens: &str) -> &'a str {
    let start = source
        .find(opens)
        .unwrap_or_else(|| panic!("`{opens}` is gone"));
    let rest = &source[start..];
    &rest[..rest
        .find("\"#;")
        .unwrap_or_else(|| panic!("`{opens}` never closes"))]
}

/// A page refusal is `Unsent` only by a code its script answers before its
/// first event (t-66200): in each input body every refusal before the first
/// statement that sends the page anything — a code, or the selector's own —
/// is in the table, and no refusal after it names one. Bringing a control
/// into view is not sending it an event; a scroll's own move is its first.
#[test]
fn a_pages_unsent_refusals_come_before_its_first_event() {
    let table = block_after(BROWSER, "pub(crate) const UNSENT_PAGE_CODES:");
    let unsent: Vec<&str> = table
        .lines()
        .skip(1)
        .filter_map(|line| line.trim().strip_prefix('"')?.strip_suffix("\","))
        .collect();
    assert!(
        unsent.contains(&"element_obscured") && unsent.len() == 12,
        "{unsent:?}"
    );
    for (opens, sends) in [
        (
            "pub(crate) const CLICK_BODY: &str",
            &["dispatchEvent(", ".focus(", ".click()"][..],
        ),
        (
            "pub(crate) const TYPE_BODY: &str",
            &[
                "dispatchEvent(",
                ".focus(",
                "execCommand(",
                ".select()",
                ".selected = true",
                ".set.call(",
                "textContent =",
            ][..],
        ),
        (
            "const BROWSER_SCROLL_BODY: &str",
            &["scrollIntoView(", "scrollTo(", "scrollBy("][..],
        ),
    ] {
        let script = raw_body(BROWSER, opens);
        let first = sends
            .iter()
            .filter_map(|send| script.find(send))
            .min()
            .unwrap_or_else(|| panic!("{opens} sends nothing"));
        let (before, after) = script.split_at(first);
        let refusals = before
            .split("zcFail(")
            .skip(1)
            .map(|rest| rest.split(')').next().unwrap_or_default().trim_matches('"'));
        for refusal in refusals {
            assert!(
                refusal == "selected.code" || unsent.contains(&refusal),
                "{opens}: `{refusal}` before the first event is not in the table"
            );
        }
        for code in &unsent {
            assert!(
                !after.contains(&format!("zcFail(\"{code}\")")),
                "{opens}: `{code}` is answered after the first event"
            );
        }
    }
}

/// What an executor refused before it sent anything it says `Unsent` itself
/// (t-66200) — a mark whose pin broke on the read-only re-measure, a form a
/// fill wrote nothing to, the keys held at a password field — and every input
/// road judges the page's answer by the one table.
#[test]
fn an_executor_says_unsent_for_what_it_refused_before_sending() {
    let form = include_str!("../../src/cmd/browser/form.rs");
    let marked = block_after(BROWSER, "async fn press_mark(");
    let pinned = &marked[marked.find("look_still_holds(&mark").unwrap()
        ..marked.find("press(&pane, &mark.selector").unwrap()];
    assert!(pinned.contains(".map_err(|error| BrowserRefusal::Unsent(error.message))?"));
    let fill = block_after(form, "pub(crate) async fn automate_fill(");
    let stale = &fill[fill.find("if report.stale").unwrap()..fill.find("report.against(").unwrap()];
    assert!(
        stale.contains("return Err(BrowserRefusal::Unsent("),
        "{stale}"
    );
    let typed = block_after(BROWSER, "pub(crate) fn typed_report(");
    let held = typed.find("SecretEntryVerdict::PersonsEntry {").unwrap();
    assert!(typed[held..].starts_with("SecretEntryVerdict::PersonsEntry {\n            return Err(BrowserRefusal::Unsent(page_failure("));
    for executor in [
        "async fn press(",
        "pub(crate) async fn automate_type(",
        "pub(crate) async fn automate_scroll(",
    ] {
        let body = block_after(BROWSER, executor);
        assert!(
            body.contains("input_value(reply)?") && !body.contains("page_value(reply)"),
            "{executor}"
        );
    }
}

/// Who lifts a paused work browser rides every refusal an agent hears
/// (t-66200): the backend says `workspace_paused` only through its one
/// formatter, which carries `[resume=…]`, and the door answers a work
/// browser's refusals — a look's completion's too — through `door_refused`,
/// which moves the word into `error.resume` for a `--json` call.
#[test]
fn every_paused_refusal_names_who_lifts_it() {
    let source = include_str!("../../src/browser_workspace.rs");
    let production = &source[source.find("const WORKSPACE_CAP").unwrap()
        ..source.find("mod review_regressions").unwrap()];
    assert!(
        !production.contains("\"workspace_paused"),
        "a paused refusal written by hand carries no resume word"
    );
    let formatter = block_after(production, "fn paused(");
    assert!(formatter.contains("{WORKSPACE_PAUSED}[resume={}]: "));
    let answer = block_after(RUNTIME, "async fn answer_browser_command(");
    assert_eq!(
        answer.matches("door_refused(argv, &error)").count(),
        5,
        "the door's three refusals, a look its tab was taken from (t-67064) and a look's completion: {answer}"
    );
    let door = block_after(RUNTIME, "pub(super) fn door_refused(");
    for piece in [
        "paused_resume(error)",
        "person_controlled_words(error)",
        "if answers_json(argv)",
        "\"resume\": resume",
    ] {
        assert!(door.contains(piece), "{piece}");
    }
    assert!(
        block_after(RUNTIME, "pub(super) fn stamp_browser_observation(")
            .contains("answers_json(argv)")
    );
}

/// The session's regrant is the bar's (t-66200): only the bar's own
/// 「다시 맡기기」 asks for it — the band's resume hands back without it — the
/// chip shows what the backend says, its off switch is one command of its
/// own, and the backend keeps it with the tab alone.
#[test]
fn the_sessions_regrant_is_turned_on_by_the_bars_hand_back_alone() {
    let browser = include_str!("../../../../ui/shell-browser.js");
    let asking = block_after(browser, "async function setBrowserWorkspaceMode(");
    assert!(asking.contains("...(regrant && mode === \"agent\" ? { regrant: true } : {}),"));
    assert!(browser.contains(
        "else void setBrowserWorkspaceMode(tab, name === \"pause\" ? \"paused\" : name, { regrant: name === \"agent\" });"
    ));
    let resume = block_after(browser, "function resumeAgentBrowserWorkspaces(");
    assert!(
        !resume.contains("regrant"),
        "the band's resume hands back without the regrant"
    );
    let chip = block_after(browser, "function paintBrowserWorkspaceRegrant(");
    assert!(chip.contains("row.hidden = tab.workspace?.regrant !== true;"));
    assert_eq!(
        browser
            .matches("invoke(\"end_browser_workspace_regrant\"")
            .count(),
        1
    );
    let command = block_after(CONTROL, "async fn set_browser_workspace_mode(");
    assert!(command.contains(".regranting(regrant == Some(true))"));
    let ending = block_after(CONTROL, "pub(crate) fn end_browser_workspace_regrant(");
    assert!(
        ending.contains("from_the_main_webview(&webview)?")
            && ending.contains(".end_regrant(&label)?")
    );
    let main = include_str!("../../src/main.rs");
    assert!(main.contains("cmd::browser_workspace::end_browser_workspace_regrant,"));
    let source = include_str!("../../src/browser_workspace.rs");
    let finishing = block_after(
        source,
        "pub(crate) fn finish(mut self) -> Result<Snapshot, String> {",
    );
    assert!(
        finishing.contains("Mode::Agent => state.regrant |= self.regrant,")
            && finishing.contains("Mode::Human => state.regrant = false,")
    );
}

/// The agent goes on by its own look and hears who holds its tab (t-67064):
/// the door waits for a look's page only while its tab stays in the agent's
/// hands, stamps on a completed change's answer the revision it leaves,
/// tells a look its page did not answer what to do next, says each work
/// browser's holder on `tabs` with none of a held tab's page, and the bar's
/// handoff tells the agent holding the tab once it finished — in the
/// workspace module's own words.
#[test]
fn the_agent_goes_on_by_its_own_look_and_hears_who_holds_its_tab() {
    let answer = block_after(RUNTIME, "async fn answer_browser_command(");
    let dispatching = answer.find("let dispatching = async {").unwrap();
    let waited = answer
        .find("crate::browser_workspace::unless_taken(operation.as_ref(), dispatching)")
        .unwrap();
    assert!(dispatching < waited);
    let settled = answer.find("operation.settle(dispatched)").unwrap();
    let stamped = answer
        .find("stamp_browser_change(&mut answer, revision)")
        .unwrap();
    assert!(settled < stamped);
    assert!(answer.contains(".after_failed_look("));
    let dispatch = block_after(RUNTIME, "async fn dispatch_browser_command(");
    let listing = block_after(dispatch, "(\"tabs\", 1) => {");
    assert!(listing.contains(".standing_agents(&label, pane)"));
    assert_eq!(listing.matches(".filter(|_| shown)").count(), 2);
    let command = block_after(CONTROL, "async fn set_browser_workspace_mode(");
    let finished = command.find("transition.finish()?").unwrap();
    let told = command.find("tell_handed_back(&snapshot)").unwrap();
    assert!(finished < told);
    let telling = block_after(CONTROL, "fn tell_handed_back(");
    for piece in [
        "Mode::Agent",
        "crate::browser_workspace::handed_back(",
        "crate::orchestration::tell_seat_of_term(",
        "spawn_blocking(",
    ] {
        assert!(telling.contains(piece), "{piece}");
    }
}
