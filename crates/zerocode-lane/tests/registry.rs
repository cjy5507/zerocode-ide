//! The registry drives **real** child processes, because the two rules it
//! exists for are both about timing: a background lane must still be reading
//! while nobody looks at it, and the moment someone looks, what they get must
//! describe the screen as it is now rather than the journey it took.
//!
//! `sh` stands in for `zo` here. Nothing in this layer knows what it is hosting,
//! which is the same reason the IDE still has a terminal on a machine with no
//! agent installed.

#![cfg(unix)]

use std::time::{Duration, Instant};

use zerocode_core::{ALL_AGENTS, Lane, LaneState, PaneKey};
use zerocode_lane::{LaneEvent, LaneRegistry, PtyHandle, RegistryError};
use zerocode_pty::{Cell, GridDelta, PtyLane};

fn lane() -> Lane {
    Lane::new(ALL_AGENTS[0], PaneKey::random())
}

fn spawn_with_pid(script: &str, rows: u16, cols: u16) -> (PtyHandle, u32) {
    let lane = PtyLane::spawn(
        "sh",
        &["-c".to_string(), script.to_string()],
        None,
        &[],
        rows,
        cols,
    )
    .expect("spawn sh");
    let pid = lane.pid().expect("a running child has a pid");
    (PtyHandle::from(lane), pid)
}

fn spawn(script: &str, rows: u16, cols: u16) -> PtyHandle {
    spawn_with_pid(script, rows, cols).0
}

/// The ceiling for a wait that has a condition.
///
/// Generous on purpose. A `pump_until` with a real predicate returns the moment
/// the predicate holds, so this number costs nothing on a healthy run — it is only
/// how long the test is willing to wait before calling it a failure. The tests
/// here drive REAL child processes, and under a loaded `cargo test --workspace`
/// a two- or three-second ceiling is a coin flip rather than a bound: two of them
/// failed that way in one session while passing alone every time.
///
/// A wait with `|_| false` is a different thing — it collects whatever arrives in
/// a fixed window and asserts a property of all of it. Those stay short: a slow
/// machine collects fewer frames and the assertion still holds, so the cost is
/// coverage rather than a false failure.
const SETTLES_WITHIN: Duration = Duration::from_secs(20);

/// Pump until `done` or the deadline, collecting everything seen.
fn pump_until(
    registry: &mut LaneRegistry,
    budget: Duration,
    mut done: impl FnMut(&[LaneEvent]) -> bool,
) -> Vec<LaneEvent> {
    pump_until_observed(registry, budget, |_, seen| done(seen))
}

/// [`pump_until`] when the condition is a fact held by the registry rather
/// than one event. Keeping the one pump loop here lets timing-sensitive tests
/// wait for the state they claim to snapshot instead of treating the first
/// output byte as proof that all output has arrived.
fn pump_until_observed(
    registry: &mut LaneRegistry,
    budget: Duration,
    mut done: impl FnMut(&LaneRegistry, &[LaneEvent]) -> bool,
) -> Vec<LaneEvent> {
    let deadline = Instant::now() + budget;
    let mut seen = Vec::new();
    while Instant::now() < deadline {
        seen.extend(registry.pump());
        if done(registry, &seen) {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    seen
}

fn screens(events: &[LaneEvent]) -> Vec<&GridDelta> {
    events
        .iter()
        .filter_map(|event| match event {
            LaneEvent::Screen { delta, .. } => Some(delta),
            LaneEvent::Updated(_) | LaneEvent::ClipboardWrite { .. } => None,
        })
        .collect()
}

fn updates(events: &[LaneEvent]) -> Vec<&Lane> {
    events
        .iter()
        .filter_map(|event| match event {
            LaneEvent::Updated(lane) => Some(lane),
            LaneEvent::Screen { .. } | LaneEvent::ClipboardWrite { .. } => None,
        })
        .collect()
}

/// A consumer that obeys the shift contract, exactly as the view layer must.
struct Mirror {
    rows: Vec<Vec<Cell>>,
    cols: usize,
}

impl Mirror {
    fn new(rows: usize, cols: usize) -> Self {
        Self {
            rows: vec![vec![Cell::default(); cols]; rows],
            cols,
        }
    }

    fn apply(&mut self, delta: &GridDelta) {
        if delta.full {
            let (rows, cols) = delta.size;
            self.rows = vec![vec![Cell::default(); cols]; rows];
            self.cols = cols;
        } else {
            for _ in 0..delta.scrolled_lines {
                self.rows.remove(0);
                self.rows.push(vec![Cell::default(); self.cols]);
            }
        }
        for row in &delta.rows {
            self.rows[row.index] = row.cells.clone();
        }
    }

    fn text(&self) -> String {
        let mut lines: Vec<String> = self
            .rows
            .iter()
            .map(|cells| {
                cells
                    .iter()
                    .map(|cell| cell.ch)
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect();
        while lines.last().is_some_and(String::is_empty) {
            lines.pop();
        }
        lines.join("\n")
    }
}

// ------------------------------------------------------------------ the rule

/// The reason this layer exists. Eight lanes streaming at once must not cost
/// eight screens per frame, so only the lane on the stage is serialized.
#[test]
fn only_the_focused_lane_pays_for_its_cells() {
    let mut registry = LaneRegistry::new();
    let front = registry
        .insert(lane(), spawn("printf 'front output'; sleep 3", 6, 40))
        .expect("insert");
    let back = registry
        .insert(lane(), spawn("printf 'back output'; sleep 3", 6, 40))
        .expect("insert");

    assert_eq!(
        registry.focused(),
        Some(front),
        "the first lane takes focus"
    );

    // Wait for both halves of the claim, or neither assertion below means
    // anything: the front lane has painted, and the back lane has run at all.
    let events = pump_until(&mut registry, SETTLES_WITHIN, |seen| {
        !screens(seen).is_empty() && updates(seen).iter().any(|lane| lane.id == back)
    });

    assert!(!screens(&events).is_empty(), "the focused lane must paint");
    assert!(
        updates(&events).iter().any(|lane| lane.id == back),
        "the background lane never ran, so this test proved nothing: {events:?}"
    );
    assert!(
        !events.iter().any(|event| match event {
            LaneEvent::Screen { lane, .. } => *lane == back,
            LaneEvent::Updated(_) | LaneEvent::ClipboardWrite { .. } => false,
        }),
        "a background lane must not produce cells: {events:?}"
    );
}

/// The background lane is still *reading* — it just is not serialized. If it
/// were not pumped, switching to it would show a screen frozen at the moment it
/// lost focus.
#[test]
fn a_background_lane_keeps_reading_while_nobody_looks() {
    let mut registry = LaneRegistry::new();
    let _front = registry
        .insert(lane(), spawn("sleep 3", 6, 40))
        .expect("insert");
    let back = registry
        .insert(lane(), spawn("printf 'grew while hidden'; sleep 3", 6, 40))
        .expect("insert");

    // Give the hidden lane time to produce, with no focus on it.
    pump_until_observed(&mut registry, SETTLES_WITHIN, |registry, _| {
        registry
            .screen_text(back)
            .is_some_and(|screen| screen.contains("grew while hidden"))
    });
    assert!(
        registry
            .screen_text(back)
            .is_some_and(|screen| screen.contains("grew while hidden")),
        "the hidden lane had not read its output before focus moved"
    );

    let LaneEvent::Screen { delta, .. } = registry.focus(back).expect("focus") else {
        panic!("focusing must hand back a screen");
    };
    let text: String = delta
        .rows
        .iter()
        .map(|row| row.cells.iter().map(|cell| cell.ch).collect::<String>())
        .collect();
    assert!(
        text.contains("grew while hidden"),
        "the hidden lane's output must already be on its grid: {text:?}"
    );
}

/// Focusing hands back the whole screen, because the view has never held this
/// lane's buffer and a delta from an unknown starting point means nothing.
#[test]
fn focusing_a_lane_hands_back_a_full_frame() {
    let mut registry = LaneRegistry::new();
    let front = registry
        .insert(lane(), spawn("sleep 3", 8, 30))
        .expect("insert");
    let back = registry
        .insert(lane(), spawn("printf 'hello'; sleep 3", 8, 30))
        .expect("insert");

    pump_until(&mut registry, SETTLES_WITHIN, |seen| {
        !screens(seen).is_empty()
    });

    let LaneEvent::Screen { lane, delta } = registry.focus(back).expect("focus") else {
        panic!("expected a screen");
    };
    assert_eq!(lane, back);
    assert!(delta.full, "a newly focused lane gets an absolute frame");
    assert_eq!(delta.scrolled_lines, 0, "there is nothing to shift onto");
    assert_eq!(delta.rows.len(), 8, "every row, not just the changed ones");
    assert_eq!(registry.focused(), Some(back));
    let _ = front;
}

/// **The subtle one.** A background lane accumulates scrolls nobody collected.
/// If focusing snapshotted without discarding that pending delta, the very next
/// delta would carry a scroll count the consumer already accounted for, and it
/// would shift a buffer that was already current — silently corrupting the
/// screen a few frames after every switch.
#[test]
fn a_switch_does_not_replay_the_scrolling_that_happened_off_stage() {
    let mut registry = LaneRegistry::new();
    let front = registry
        .insert(lane(), spawn("sleep 4", 6, 30))
        .expect("insert");
    let back = registry
        .insert(
            lane(),
            // Far more lines than the screen holds, so the grid scrolls repeatedly
            // while nobody is collecting deltas.
            spawn(
                "for i in $(seq 1 40); do echo \"line $i\"; done; sleep 4",
                6,
                30,
            ),
        )
        .expect("insert");

    pump_until_observed(&mut registry, SETTLES_WITHIN, |registry, _| {
        registry
            .screen_text(back)
            .is_some_and(|screen| screen.contains("line 40"))
    });
    assert!(
        registry
            .screen_text(back)
            .is_some_and(|screen| screen.contains("line 40")),
        "the off-stage scroll had not finished before focus moved"
    );

    let mut mirror = Mirror::new(6, 30);
    let LaneEvent::Screen { delta, .. } = registry.focus(back).expect("focus") else {
        panic!("expected a screen");
    };
    mirror.apply(&delta);

    // Everything the lane does from here must land on the mirror correctly.
    let after = pump_until(&mut registry, Duration::from_secs(1), |_| false);
    for delta in screens(&after) {
        assert_eq!(
            delta.scrolled_lines, 0,
            "the off-stage scrolling was already folded into the snapshot: {delta:?}"
        );
        mirror.apply(delta);
    }

    let expected = registry.screen_text(back).expect("lane still held");
    assert!(
        expected.contains("line 40"),
        "the lane should have scrolled far past its 6 rows: {expected:?}"
    );
    assert_eq!(
        mirror.text(),
        expected,
        "the mirror drifted from the real screen after a focus switch"
    );
    let _ = front;
}

// ----------------------------------------------------------------- the state

/// A pty knows two things honestly, and this is the second: the child is gone.
#[test]
fn a_child_that_exits_reports_it_once() {
    let mut registry = LaneRegistry::new();
    let only = registry
        .insert(lane(), spawn("printf 'bye'; exit 0", 6, 20))
        .expect("insert");

    let events = pump_until(&mut registry, SETTLES_WITHIN, |seen| {
        updates(seen)
            .iter()
            .any(|lane| lane.state == LaneState::Exited)
    });

    let exits: Vec<&Lane> = updates(&events)
        .into_iter()
        .filter(|lane| lane.state == LaneState::Exited)
        .collect();
    assert_eq!(exits.len(), 1, "one transition, not one per frame");
    assert_eq!(exits[0].id, only);
    assert!(exits[0].state.is_terminal());
}

/// The rail paints every lane, so a background lane's title has to reach it —
/// even though its cells never do.
#[test]
fn a_background_lane_still_reports_its_title() {
    let mut registry = LaneRegistry::new();
    let _front = registry
        .insert(lane(), spawn("sleep 3", 6, 30))
        .expect("insert");
    let back = registry
        .insert(
            lane(),
            spawn("printf '\\033]0;drain gate\\007'; sleep 3", 6, 30),
        )
        .expect("insert");

    let events = pump_until(&mut registry, SETTLES_WITHIN, |seen| {
        updates(seen)
            .iter()
            .any(|lane| lane.id == back && lane.title == "drain gate")
    });

    assert!(
        updates(&events)
            .iter()
            .any(|lane| lane.id == back && lane.title == "drain gate"),
        "the spine needs the title of a lane it is not showing"
    );
    assert!(
        screens(&events).is_empty()
            || events.iter().all(|event| match event {
                LaneEvent::Screen { lane, .. } => *lane != back,
                LaneEvent::Updated(_) | LaneEvent::ClipboardWrite { .. } => true,
            }),
        "reporting a title must not start serializing the lane"
    );
}

#[test]
fn a_background_lane_still_reports_a_bounded_osc_fifty_two_write() {
    let mut registry = LaneRegistry::new();
    let _front = registry
        .insert(lane(), spawn("sleep 3", 6, 30))
        .expect("insert");
    let back = registry
        .insert(
            lane(),
            spawn(
                "printf '\\033]52;c;ZnJvbSBiYWNrZ3JvdW5k\\007'; sleep 3",
                6,
                30,
            ),
        )
        .expect("insert");

    let events = pump_until(&mut registry, SETTLES_WITHIN, |seen| {
        seen.iter().any(|event| {
            matches!(event, LaneEvent::ClipboardWrite { lane, text }
                if *lane == back && text == "from background")
        })
    });

    assert!(events.iter().any(|event| {
        matches!(event, LaneEvent::ClipboardWrite { lane, text }
            if *lane == back && text == "from background")
    }));
    assert!(
        events
            .iter()
            .all(|event| { !matches!(event, LaneEvent::Screen { lane, .. } if *lane == back) }),
        "the side effect must not start serialising a hidden screen"
    );
}

/// The states a pty cannot observe are set from the structured channel, and
/// setting one to what it already is must not spam the view.
#[test]
fn the_states_a_pty_cannot_see_are_set_from_outside() {
    let mut registry = LaneRegistry::new();
    let only = registry
        .insert(lane(), spawn("sleep 3", 6, 20))
        .expect("insert");

    let first = registry
        .set_state(only, LaneState::AwaitingPermission)
        .expect("known lane");
    assert!(
        matches!(first, Some(LaneEvent::Updated(lane)) if lane.state == LaneState::AwaitingPermission)
    );

    let again = registry
        .set_state(only, LaneState::AwaitingPermission)
        .expect("known lane");
    assert!(again.is_none(), "an unchanged state is not an event");
    assert!(registry.lane(only).expect("held").state.needs_attention());
}

/// A lane stopped on a permission gate keeps **redrawing** — the prompt blinks,
/// the TUI repaints. So "bytes are arriving" and "the agent is making progress"
/// are different claims, and letting the first overwrite the second would erase
/// the one state a person must never miss, right as they need to see it.
#[test]
fn output_does_not_overwrite_a_lane_that_is_waiting_for_a_human() {
    let mut registry = LaneRegistry::new();
    let only = registry
        .insert(
            lane(),
            spawn("while true; do printf 'redraw'; sleep 0.05; done", 6, 30),
        )
        .expect("insert");

    registry
        .set_state(only, LaneState::AwaitingPermission)
        .expect("known lane");

    // Pump through plenty of output while the gate is up.
    pump_until(&mut registry, Duration::from_millis(600), |_| false);

    assert_eq!(
        registry.lane(only).expect("held").state,
        LaneState::AwaitingPermission,
        "a redraw must not be mistaken for progress"
    );
}

/// The gate lifting is also the structured channel's call, and the lane goes
/// back to whatever its output says it is doing.
#[test]
fn clearing_the_gate_returns_the_lane_to_what_its_output_shows() {
    let mut registry = LaneRegistry::new();
    let only = registry
        .insert(
            lane(),
            spawn("while true; do printf 'work'; sleep 0.05; done", 6, 30),
        )
        .expect("insert");

    registry
        .set_state(only, LaneState::AwaitingPermission)
        .expect("known lane");
    registry
        .set_state(only, LaneState::Streaming)
        .expect("known lane");

    // Waited for, not slept through. This used to pump a flat 400ms and then
    // assert, which is a flake by construction: the assertion is about a child
    // process's output, and under a loaded workspace run that child can be a
    // long way from having produced any. The condition is the same; only the
    // clock is now generous, and a real regression still fails — it just takes
    // the ceiling to do it.
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        registry.pump();
        if registry.lane(only).expect("held").state == LaneState::Streaming {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    assert_eq!(
        registry.lane(only).expect("held").state,
        LaneState::Streaming,
        "with the gate cleared, output drives the state again"
    );
}

// ------------------------------------------------------------------ lifetime

/// Closing a pane must not leave an agent running and spending tokens on a
/// screen nobody will ever read.
#[test]
fn removing_a_lane_kills_the_process_behind_it() {
    let mut registry = LaneRegistry::new();
    let first = registry
        .insert(lane(), spawn("sleep 30", 6, 20))
        .expect("insert");
    let second = registry
        .insert(lane(), spawn("sleep 30", 6, 20))
        .expect("insert");
    assert_eq!(registry.len(), 2);

    let removed = registry.remove(first).expect("removed");
    assert!(!removed.orphaned, "a live child must actually be killed");
    assert_eq!(registry.len(), 1);
    assert_eq!(
        registry.focused(),
        Some(second),
        "focus moves rather than pointing at a lane that is gone"
    );
    assert!(
        registry.remove(first).is_none(),
        "removing twice is a no-op"
    );
}

/// **The gap the focus contract had.** Closing the lane on the stage moves the
/// stage — and a move that skipped the snapshot would leave the view applying
/// increments, including a scroll count accumulated off-stage, against the
/// buffer of a lane that no longer exists.
#[test]
fn closing_the_focused_lane_hands_back_the_new_stage_as_a_full_frame() {
    let mut registry = LaneRegistry::new();
    let front = registry
        .insert(lane(), spawn("printf 'front'; sleep 4", 6, 30))
        .expect("insert");
    let back = registry
        .insert(
            lane(),
            // Scrolls far past its six rows while nobody collects its deltas.
            spawn(
                "for i in $(seq 1 40); do echo \"line $i\"; done; sleep 4",
                6,
                30,
            ),
        )
        .expect("insert");

    pump_until_observed(&mut registry, SETTLES_WITHIN, |registry, _| {
        registry
            .screen_text(back)
            .is_some_and(|screen| screen.contains("line 40"))
    });
    assert!(
        registry
            .screen_text(back)
            .is_some_and(|screen| screen.contains("line 40")),
        "the off-stage scroll had not finished before the focused lane closed"
    );

    let removed = registry.remove(front).expect("removed");
    let Some(LaneEvent::Screen { lane, delta }) = removed.refocused else {
        panic!("closing the focused lane must hand back the new stage");
    };
    assert_eq!(lane, back);
    assert!(delta.full, "the view has never held this lane's buffer");
    assert_eq!(delta.scrolled_lines, 0, "there is nothing to shift onto");

    // And the off-stage scrolling must not come back on the next frames.
    let mut mirror = Mirror::new(delta.size.0, delta.size.1);
    mirror.apply(&delta);
    for delta in screens(&pump_until(&mut registry, Duration::from_secs(1), |_| {
        false
    })) {
        assert_eq!(
            delta.scrolled_lines, 0,
            "the snapshot already folded that scrolling in: {delta:?}"
        );
        mirror.apply(delta);
    }
    assert_eq!(
        mirror.text(),
        registry.screen_text(back).expect("lane still held"),
        "the mirror drifted after the stage moved"
    );
}

/// Closing a lane that was not on the stage changes nothing about the stage, so
/// there is no frame to hand back.
#[test]
fn closing_a_background_lane_leaves_the_stage_alone() {
    let mut registry = LaneRegistry::new();
    let front = registry
        .insert(lane(), spawn("sleep 3", 6, 20))
        .expect("insert");
    let back = registry
        .insert(lane(), spawn("sleep 3", 6, 20))
        .expect("insert");

    let removed = registry.remove(back).expect("removed");
    assert!(
        removed.refocused.is_none(),
        "the stage did not move, so there is nothing to repaint"
    );
    assert_eq!(registry.focused(), Some(front));
}

#[test]
fn closing_the_last_lane_leaves_no_stage_at_all() {
    let mut registry = LaneRegistry::new();
    let only = registry
        .insert(lane(), spawn("sleep 3", 6, 20))
        .expect("insert");

    let removed = registry.remove(only).expect("removed");
    assert!(removed.refocused.is_none());
    assert_eq!(registry.focused(), None);
    assert!(registry.is_empty());
}

/// A child that finished on its own was never killed, and reporting it orphaned
/// would cry wolf about every lane a person let run to completion.
#[test]
fn a_lane_whose_child_already_finished_is_not_reported_as_orphaned() {
    let mut registry = LaneRegistry::new();
    let only = registry
        .insert(lane(), spawn("exit 0", 6, 20))
        .expect("insert");

    pump_until(&mut registry, SETTLES_WITHIN, |seen| {
        updates(seen)
            .iter()
            .any(|lane| lane.state == LaneState::Exited)
    });

    let removed = registry.remove(only).expect("removed");
    assert!(
        !removed.orphaned,
        "the child was already gone, so there was nothing to fail at killing"
    );
}

/// Holding one id twice would leave `len()` and `lanes()` disagreeing with what
/// the registry actually has, and would drop a running child without killing
/// it. Refusing keeps both the rail and the process honest.
#[test]
fn inserting_an_id_the_registry_already_holds_is_refused() {
    let mut registry = LaneRegistry::new();
    let lane = lane();
    let id = registry
        .insert(lane.clone(), spawn("sleep 3", 6, 20))
        .expect("first insert");

    // The refused lane was *moved* in, so a registry that neither keeps nor
    // kills it leaves the caller with no handle at all — the process would run
    // until the machine is rebooted.
    let (refused, refused_pid) = spawn_with_pid("sleep 30", 6, 20);
    let error = registry
        .insert(lane, refused)
        .expect_err("a duplicate id must be refused");
    assert!(
        matches!(error, RegistryError::DuplicateLane(duplicate) if duplicate == id),
        "unexpected error: {error}"
    );

    assert_eq!(registry.len(), 1, "the rail must not grow on a refusal");
    assert_eq!(
        registry.lanes().count(),
        1,
        "and it must not list the same lane twice"
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && process_is_alive(refused_pid) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(
        !process_is_alive(refused_pid),
        "pid {refused_pid} was refused but left running"
    );
}

/// Ask the operating system, not the registry, whether a process is still there.
fn process_is_alive(pid: u32) -> bool {
    std::process::Command::new("ps")
        .args(["-p", &pid.to_string()])
        .output()
        .expect("run ps")
        .status
        .success()
}

/// The same leak at registry scale: closing a window drops the registry, and
/// eight agents must not outlive it.
#[test]
fn dropping_the_registry_takes_every_child_with_it() {
    let mut registry = LaneRegistry::new();
    let mut pids = Vec::new();
    for _ in 0..3 {
        let (pty, pid) = spawn_with_pid("sleep 30", 4, 20);
        pids.push(pid);
        registry.insert(lane(), pty).expect("insert");
    }
    assert!(
        pids.iter().all(|pid| process_is_alive(*pid)),
        "the fixture never started: {pids:?}"
    );

    drop(registry);

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && pids.iter().any(|pid| process_is_alive(*pid)) {
        std::thread::sleep(Duration::from_millis(20));
    }
    let survivors: Vec<u32> = pids
        .into_iter()
        .filter(|pid| process_is_alive(*pid))
        .collect();
    assert!(
        survivors.is_empty(),
        "closing the window left agents running: {survivors:?}"
    );
}

#[test]
fn lanes_keep_the_order_they_were_added_in() {
    let mut registry = LaneRegistry::new();
    let ids: Vec<_> = (0..3)
        .map(|_| {
            registry
                .insert(lane(), spawn("sleep 2", 4, 20))
                .expect("insert")
        })
        .collect();

    let listed: Vec<_> = registry.lanes().map(|lane| lane.id).collect();
    assert_eq!(listed, ids, "the rail must not shuffle under the user");
}
