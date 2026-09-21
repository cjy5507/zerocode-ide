//! §2.2 of `docs/design/quota-aware-summoning-and-handover.md`: the quota
//! wall is judged by TWO witnesses, asked beside the stall probe's evidence
//! and outside the team table, and the ledger takes neither alone.

use std::path::Path;

fn shell_source(name: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    std::fs::read_to_string(src.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

fn core_source(name: &str) -> String {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("../zerocode-core/src");
    std::fs::read_to_string(src.join(name)).unwrap_or_else(|why| panic!("{name}: {why}"))
}

/// The wall witness rides the stall sweep: asked only of a pane the stall
/// probe found quiet, after the team table is given back, through the pure
/// two-witness function, and written down through the actor beside the
/// quiet sweep it replaces for that pane. The stall probe's four pieces of
/// evidence — hook `Working`, PTY `last_output_at`, `QUIET_GRACE_MS`, the
/// never-output fallback — stand as they were.
#[test]
fn the_wall_witness_is_asked_beside_the_stall_probe_and_outside_the_team_table() {
    let orchestration = shell_source("orchestration.rs");
    let sweep = super::support::block_after(&orchestration, "fn notify_stalled_workers(");
    let table_given_back = sweep
        .find("drop(teams);")
        .expect("the sweep gives the team table back");
    let stall_probe = sweep
        .find("host.quiet_since(")
        .expect("the stall probe is still asked");
    // Formatting-stable: rustfmt may break `host` and the method apart.
    let wall_probe = sweep
        .find(".quota_wall_marker(one.term, &one.agent)")
        .expect("the wall witness is asked of the host");
    assert!(
        table_given_back < stall_probe && table_given_back < wall_probe,
        "a host probe runs under the team table:\n{sweep}"
    );
    assert!(
        wall_probe > stall_probe,
        "the wall is asked of a pane the stall probe has not found quiet"
    );
    for needed in [
        "zerocode_core::orchestration::quota_wall_witness(",
        "usage_headroom(&held.usage,",
        "held.actor.quota_walls(",
        "held.actor.quiet_sweep(",
        "walled_already",
    ] {
        assert!(
            sweep.contains(needed),
            "the sweep lost `{needed}`:\n{sweep}"
        );
    }
    assert!(
        !sweep.contains("end_attempt(") && !sweep.contains("worker-stop"),
        "the witness settles something"
    );

    let tools = shell_source("agent_tools_runtime.rs");
    let stall = super::support::block_after(
        &tools,
        "fn quiet_since(&self, term: TermId, worker_started_ms: i64, now_ms: i64) -> Option<i64> {",
    );
    for evidence in [
        "HookState::Working",
        "last_output_epoch_ms()",
        "quiet_since_output(",
        "worker_started_ms",
    ] {
        assert!(
            stall.contains(evidence),
            "the stall probe lost `{evidence}`:\n{stall}"
        );
    }
    let quiet = super::support::block_after(&tools, "fn quiet_since_output(");
    assert!(quiet.contains("QUIET_GRACE_MS") && quiet.contains("worker_started_ms"));
    let fenced = super::support::block_after(&tools, "fn with_quota_wall_observation(");
    for evidence in [
        "pane_states()",
        "lock_pty(&held)",
        "quiet_since_output(",
        "quota_wall::marker_for(",
        "commit(marker)",
    ] {
        assert!(
            fenced.contains(evidence),
            "the retirement fence lost {evidence}"
        );
    }
    let marker = super::support::block_after(&tools, "fn quota_wall_marker(");
    let table_first = marker
        .find("quota_wall::has_rule(")
        .expect("the real host asks the marker table");
    let grid_copied = marker
        .find("self.capture(")
        .expect("the real host reads the screen");
    assert!(
        table_first < grid_copied,
        "the grid is copied before the table is asked:\n{marker}"
    );
    assert!(
        marker.contains("quota_wall::marker_for(") && marker.contains("transcript_path"),
        "the real host does not read the transcript's tail:\n{marker}"
    );

    let table = shell_source("quota_wall.rs");
    assert_eq!(
        table
            .matches("const STALL_MARKERS: &[StallMarkerRule] = &[")
            .count(),
        1,
        "the marker table is one table"
    );
    for agent in ["\"codex\"", "\"claude\"", "\"zo\""] {
        assert!(table.contains(agent), "the marker table lost {agent}");
    }

    let core = core_source("orchestration.rs");
    let witness = super::support::block_after(&core, "pub fn quota_wall_witness(");
    for needed in [
        "let marker = marker?;",
        "let headroom = headroom?;",
        "read_gauge(headroom, now_ms)",
        // At the wall AND fresh, through the one sentence every road that
        // acts on a spent gauge reads (t-4839).
        "!reading.wall_to_act_on()",
        "headroom.updated_at_ms > now_ms",
    ] {
        assert!(
            witness.contains(needed),
            "the two-witness rule lost `{needed}`:\n{witness}"
        );
    }
    // That sentence, and the two readers that would otherwise each keep their
    // own copy of it: the summons refusal, and the set a summon judgment is
    // closed over. 2026-09-19 is what two copies cost — a gauge too old to
    // refuse on was a wall in the options alone, so the agent the window went
    // on to summon was missing from the choice and the row read as a
    // disagreement.
    let sentence = super::support::block_after(&core, "fn wall_to_act_on(&self) -> bool {");
    assert!(
        sentence.contains("self.at_wall && !self.stale"),
        "the one wall sentence stopped asking for a fresh number:\n{sentence}"
    );
    let rooms = super::support::block_after(&core, "fn installed_rooms(");
    assert!(
        rooms.contains("read_gauge(held, now_ms).wall_to_act_on()"),
        "the options pass judges a wall of its own again:\n{rooms}"
    );
    let verdict = super::support::block_after(&core, "pub fn quota_verdict(");
    assert!(
        verdict.contains("if reading.at_wall && !reading.wall_to_act_on() {")
            && verdict.contains("if reading.wall_to_act_on() {"),
        "the refusal road stopped reading the one sentence:\n{verdict}"
    );
    let gauge = super::support::block_after(&core, "fn read_gauge(");
    for needed in [
        "QUOTA_POLICY.wall_percent",
        "QUOTA_POLICY.snapshot_max_age_ms",
    ] {
        assert!(
            gauge.contains(needed),
            "the shared quota gauge lost {needed}"
        );
    }
    let news = super::support::block_after(&core, "pub fn workers_quota_walled(");
    assert!(
        news.contains("MessageKind::QuotaWalled") && news.contains("worker.taken_over"),
        "the notice road lost its kind or its takeover rule:\n{news}"
    );
    assert!(
        !news.contains("end_attempt(") && !news.contains("TaskStatus::"),
        "the notice settles something:\n{news}"
    );
}

/// The handover walk (§2.3) is three argv steps through the ONE door, in the
/// order `--retry-of` forces — ① the WIP commit, ② `worker-stop`, ③
/// `worker-start --retry-of … --inherit-checkout` — each named to the ledger
/// as it happens, reserved before the first and settled after the last;
/// it rides the beat that already exists, outside every lock, and no second
/// ledger API grows beside the verbs.
#[test]
fn the_handover_walk_is_three_argv_steps_through_the_one_door_in_order() {
    let orchestration = shell_source("orchestration.rs");
    let walk = super::support::block_after(&orchestration, "pub(crate) fn walk_handover(");
    let at = |needle: &str| {
        walk.find(needle)
            .unwrap_or_else(|| panic!("the handover walk lost `{needle}`:\n{walk}"))
    };
    let reserved = at("actor.handover_begin(");
    let committed = at("door.wip_commit(");
    // The recap is read while the walled pane still stands: ② closes it, and
    // with it the window's record of which transcript was the worker's.
    let recapped = at(".predecessor_tail(plan)");
    let stopped = at("\"worker-stop\".to_string()");
    let started = at("\"worker-start\".to_string()");
    // The LAST settle: the first is the `failed` closure, defined before the
    // steps so a refused step can settle without repeating itself.
    let settled = walk
        .rfind("actor.handover_settle(")
        .expect("the handover walk lost its settle");
    assert!(
        reserved < committed
            && committed < recapped
            && recapped < stopped
            && stopped < started
            && started < settled,
        "the steps are out of order:\n{walk}"
    );
    for needed in [
        "\"--retry-of\".to_string()",
        "\"--inherit-checkout\".to_string()",
        "\"--reason\".to_string()",
        "\"quota-wall\".to_string()",
        "\"--retry-request\".to_string()",
        "handover_paragraph(plan,",
        "actor.handover_step(",
        "HANDOVER_STEPS",
    ] {
        assert!(walk.contains(needed), "the handover walk lost `{needed}`");
    }
    for forbidden in [
        ".end_attempt(",
        ".prepare_worker_start(",
        ".start_worker(",
        "actor.plan(",
        ".post(",
    ] {
        assert!(
            !walk.contains(forbidden),
            "the handover walk reaches the ledger without the argv road: `{forbidden}`"
        );
    }
    let door = super::support::block_after(&orchestration, "impl HandoverDoor for LiveDoor<'_> {");
    assert!(
        door.contains("crate::quota_wall::wip_commit(") && door.contains("run_from_seat(\n"),
        "the production door lost git or the one road:\n{door}"
    );
    let beat = super::support::block_after(&orchestration, "fn walk_handovers(");
    assert!(
        beat.contains("zerocode_core::orchestration::next_handover")
            && beat.contains("current_pane_capability(&plan.team, &plan.pane)")
            && beat.contains("drop(rows);"),
        "the beat's half plans from a snapshot and presents the seat:\n{beat}"
    );
    let dropped = beat.find("drop(rows);").expect("the snapshot is let go");
    let walked = beat.find("walk_handover(").expect("the walk");
    assert!(dropped < walked, "the walk holds the snapshot");
    let tick = super::support::block_after(&orchestration, "pub(crate) fn tick(");
    let stalled = tick
        .find("notify_stalled_workers(host, now_ms);")
        .expect("the stall sweep");
    let handed = tick
        .find("walk_handovers(host, overrides, now_ms);")
        .expect("the walk rides the beat");
    assert!(
        stalled < handed,
        "the walk runs before the wall is witnessed"
    );
    let core = core_source("orchestration.rs");
    let plan = super::support::block_after(&core, "fn handover_candidate<");
    for needed in [
        "dispatch.is_open()",
        "worker.taken_over",
        "handover_consumes_attempt",
        "QUOTA_POLICY.handover_max",
        "effective_handover_order(run, worker)",
    ] {
        assert!(
            plan.contains(needed),
            "the candidate rule lost `{needed}`:\n{plan}"
        );
    }
    let restart = super::support::block_after(&core, "pub fn window_restarted(");
    assert!(
        restart.contains("HANDOVER_INTERRUPTED") && restart.contains("deliver_receipt("),
        "a restart no longer reports a half-walked handover:\n{restart}"
    );
}

/// The guide teaches the two new flags and the two new kinds of news, in
/// the words the ledger uses — a coordinator reads this before it reads
/// `help`.
#[test]
fn the_orchestration_guide_teaches_the_handover_order_and_its_flags() {
    let skill = include_str!("../../../../skills/orchestration/SKILL.md");
    for needed in [
        "--inherit-checkout",
        "handover-policy --on-quota-wall <agent[:model[:effort]]>",
        "--wip-commit",
        "handover-policy --off",
        "`quota_walled`",
        "`handover` receipt",
        "worker-stop --reason quota-wall",
        "wip(handover):",
        "steps:",
        "at most\ntwice",
        "`interrupted`",
        "TWO witnesses",
    ] {
        assert!(
            skill.contains(needed),
            "the orchestration guide no longer teaches `{needed}`"
        );
    }
}

/// t-4537: a worker whose own transcript ends on a transient API error is
/// typed at once under a declared order. The stall-cause table is still one
/// table and its transient rows read no screen; the probe rides the stall
/// sweep after the wall's own witness and outside the team table; the
/// continuation is licensed by the SAME composer reading the mail pointer
/// uses, planned by the one pure rule, reserved before the words, typed
/// through the pointer's guarded door, and settled only on the provider's
/// prompt report within the window's one submit budget.
#[test]
fn the_transient_error_continuation_rides_the_stall_sweep_through_the_pointers_doors() {
    let table = shell_source("quota_wall.rs");
    let rows = super::support::block_after(&table, "const STALL_MARKERS: &[StallMarkerRule] = &[");
    for needed in [
        "cause: StallCause::TransientApiError",
        "errors: &[\"server_error\"]",
        "info: \"server_overloaded\"",
        "message_starts: Some(\"stream disconnected before completion\")",
    ] {
        assert!(rows.contains(needed), "the table lost `{needed}`:\n{rows}");
    }
    let transient_rows = rows
        .split("StallMarkerRule {")
        .filter(|row| row.contains("StallCause::TransientApiError"))
        .collect::<Vec<_>>();
    assert_eq!(
        transient_rows.len(),
        2,
        "one transient row per measured CLI"
    );
    for row in transient_rows {
        assert!(
            row.contains("screen: &[],"),
            "a transient row reads a screen line, which cannot say which turn it ends:\n{row}"
        );
    }

    let orchestration = shell_source("orchestration.rs");
    let sweep = super::support::block_after(&orchestration, "fn notify_stalled_workers(");
    let table_given_back = sweep
        .find("drop(teams);")
        .expect("the team table is let go");
    let wall = sweep
        .find(".quota_wall_marker(one.term, &one.agent)")
        .expect("the wall is asked");
    let transient = sweep
        .find("crate::quota_wall::transient_error_for(")
        .expect("the transient cause is asked");
    let walls_written = sweep
        .find("held.actor.quota_walls(")
        .expect("walls are written");
    let resumed = sweep
        .find("resume_stalled_workers(host,")
        .expect("the continuation rides the sweep");
    assert!(
        table_given_back < wall && wall < transient && walls_written < resumed,
        "the transient probe is out of order:\n{sweep}"
    );
    assert!(
        sweep.contains("one.resume_declared"),
        "a transcript is read for an undeclared run"
    );

    let resume = super::support::block_after(&orchestration, "fn resume_stalled_workers(");
    let at = |needle: &str| {
        resume
            .find(needle)
            .unwrap_or_else(|| panic!("the continuation lost `{needle}`:\n{resume}"))
    };
    let licensed = at("composer_at_rest(host, one.term, heard)");
    let planned = at("resume_plan(run, &one.worker, &marker, now_ms)");
    let reserved = at("actor.resume_begin(plan, now_ms)");
    let typed = at("host.point(one.term, RESUME_LINE, true)");
    assert!(
        licensed < planned && planned < reserved && reserved < typed,
        "the continuation types before it is licensed, planned or reserved:\n{resume}"
    );
    assert!(resume.contains("Some(Err(NotResumed::InFlight)) => continue"));
    for forbidden in [".send(", ".paste(", "type_prompt_at_term("] {
        assert!(
            !resume.contains(forbidden),
            "the continuation types around the pointer's guarded door: `{forbidden}`"
        );
    }
    let pointer = super::support::block_after(&orchestration, "fn point_at_waiting_mail(");
    assert!(
        pointer.contains("composer_at_rest(host, term, heard)"),
        "the pointer and the continuation read the composer apart"
    );
    let outcome = super::support::block_after(&orchestration, "fn resume_outcome(");
    for needed in [
        "if one.prompt_seen",
        "zerocode_pty::ready::SUBMIT_ACK_TIMEOUT",
        "DeliveryOutcome::Refused(why)",
    ] {
        assert!(
            outcome.contains(needed),
            "the settlement lost `{needed}`:\n{outcome}"
        );
    }
    let tick = super::support::block_after(&orchestration, "pub(crate) fn tick(");
    let settled = tick
        .find("settle_resumes(now_ms);")
        .expect("settled on the beat");
    let swept = tick
        .find("notify_stalled_workers(host, now_ms);")
        .expect("the stall sweep");
    assert!(settled < swept, "the sweep plans before the rows settle");

    let panes = shell_source("pane_runtime.rs");
    let prompt = super::support::block_after(&panes, "if report.prompt.is_some() {");
    assert!(
        prompt.contains("orchestration::pane_prompt_submitted(report.term);"),
        "the provider's prompt report no longer reaches a typed continuation:\n{prompt}"
    );

    let core = core_source("orchestration.rs");
    let plan = super::support::block_after(&core, "pub fn resume_plan(");
    for needed in [
        "Some(OnTransientError::Resume)",
        "worker.taken_over",
        "awaiting_reply(run, &worker.id)",
        "MessageKind::QuotaWalled",
        "Err(NotResumed::InFlight)",
        "RESUME_POLICY.attempts_max",
        "resume_may_have_typed(body)",
        "RESUME_POLICY.retry_after_ms",
    ] {
        assert!(
            plan.contains(needed),
            "the resume rule lost `{needed}`:\n{plan}"
        );
    }
    let begin = super::support::block_after(&core, "pub fn resume_begin(");
    assert!(
        begin.contains("resume_plan(run, &plan.worker, &plan.marker, now_ms)")
            && !begin.contains("deliver_receipt("),
        "the reservation plans apart from the beat, or delivers an unsettled row:\n{begin}"
    );
    let restart = super::support::block_after(&core, "pub fn window_restarted(");
    assert!(
        restart.contains("MessageKind::Resumed => RESUME_TYPING"),
        "a restart no longer reports a continuation it interrupted"
    );
}

/// The guide teaches the transient-error order in the words the ledger
/// uses, beside the wall's.
#[test]
fn the_orchestration_guide_teaches_the_transient_error_order() {
    let skill = include_str!("../../../../skills/orchestration/SKILL.md");
    for needed in [
        "handover-policy --on-transient-error resume",
        "`resumed` receipt",
        "`submitted`",
        "`not_submitted`",
        "`ceilingReached`",
        "`interrupted`",
        "at most three",
        "went_quiet",
    ] {
        assert!(
            skill.contains(needed),
            "the orchestration guide no longer teaches `{needed}`"
        );
    }
}
