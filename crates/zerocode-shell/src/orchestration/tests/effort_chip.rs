//! t-81532: the composer's effort chip. The word it shows is the session's own
//! record of its last turn, the summons' word before any turn; a press moves
//! a Claude Code pane one arrow a rung through the picker, for this session
//! only, between turns only; a CLI no typed key moves is shown and never
//! pressed; and a pane a person moved is never moved by the step-effort seat.

use super::*;
use crate::orchestration::step_effort::{PressRefused, pane_effort, press};
use zerocode_core::capabilities::CLAUDE_EFFORT_PICKER;
use zerocode_core::effort_chip::{EffortChip, EffortWitness, Stuck};

/// The worker's transcript as the pane's own tail reads it.
fn tail_of(stood: &StoppedWorker) -> Option<String> {
    std::fs::read_to_string(&stood.host.transcript).ok()
}

/// The picker's legend on the worker's screen, as Claude Code 2.1.278 draws it.
fn picker_up(stood: &StoppedWorker) {
    *stood.host.screen.lock().unwrap() = format!(
        "   Effort\n   low  ▲medium   high   xhigh   max   ultracode\n   ←/→ to adjust · Enter to confirm · {} · Esc to cancel\n",
        CLAUDE_EFFORT_PICKER.legend
    );
}

#[test]
fn the_chip_shows_the_summons_before_any_turn_and_the_sessions_record_after() {
    let stood = StoppedWorker::stand_with(
        98_100,
        "--agent claude --model claude-opus-5-5 --effort medium",
        "",
        Vec::new(),
    );
    let term = stood.host.worker_term;
    let before = pane_effort(&stood.host, term, None);
    assert_eq!(before.current.as_deref(), Some("medium"));
    assert_eq!(before.witness, EffortWitness::Summons);
    assert_eq!(before.stuck, Some(Stuck::NeverHeard), "no turn heard yet");
    assert!(before.rewrites_cache);

    let ran = a_progressed_turn("read it", "high");
    let owned: Vec<&str> = ran.iter().map(String::as_str).collect();
    stood.stops_on(&owned, stood.began + 5_000);
    let after = pane_effort(&stood.host, term, tail_of(&stood).as_deref());
    assert_eq!(
        after.current.as_deref(),
        Some("high"),
        "the record outranks the summons"
    );
    assert_eq!(after.witness, EffortWitness::Turn);
    assert_eq!(after.stuck, None);
    assert_eq!(after.door, Some("picker"));
}

#[test]
fn a_press_moves_a_claude_pane_for_this_session_only_and_the_word_waits_for_the_next_turn() {
    let stood = StoppedWorker::stand_with(
        98_200,
        "--agent claude --model claude-opus-5-5 --effort medium",
        "",
        Vec::new(),
    );
    let term = stood.host.worker_term;
    let ran = a_progressed_turn("read it", "medium");
    let owned: Vec<&str> = ran.iter().map(String::as_str).collect();
    stood.stops_on(&owned, stood.began + 5_000);
    picker_up(&stood);
    let tail = tail_of(&stood);
    let chip = press(&stood.host, term, tail.as_deref(), "max").expect("the press");
    assert_eq!(
        stood.sent_at_worker(),
        [
            CLAUDE_EFFORT_PICKER.open,
            "\r",
            CLAUDE_EFFORT_PICKER.raise,
            CLAUDE_EFFORT_PICKER.raise,
            CLAUDE_EFFORT_PICKER.raise,
            CLAUDE_EFFORT_PICKER.session_only,
        ],
        "the bare picker, three arrows up (medium → high → xhigh → max), then this session only"
    );
    assert!(
        stood
            .sent_at_worker()
            .iter()
            .all(|key| !key.contains("max")),
        "never `/effort max`, which saves the account's default"
    );
    assert_eq!(
        chip.current.as_deref(),
        Some("medium"),
        "the record has not moved yet"
    );
    assert_eq!(chip.pending.as_deref(), Some("max"));

    // A second press moves from the word the first put the session on.
    let again = press(&stood.host, term, tail.as_deref(), "high").expect("the second press");
    assert_eq!(again.pending.as_deref(), Some("high"));
    assert_eq!(
        &stood.sent_at_worker()[6..],
        [
            CLAUDE_EFFORT_PICKER.open,
            "\r",
            CLAUDE_EFFORT_PICKER.lower,
            CLAUDE_EFFORT_PICKER.lower,
            CLAUDE_EFFORT_PICKER.session_only,
        ],
        "from the pressed max, not the recorded medium"
    );

    // The next turn runs at it: the record says so and nothing waits.
    let mut next = ran.clone();
    next.extend(a_progressed_turn("go on", "high"));
    let owned: Vec<&str> = next.iter().map(String::as_str).collect();
    stood.stops_on(&owned, stood.began + 9_000);
    let landed = pane_effort(&stood.host, term, tail_of(&stood).as_deref());
    assert_eq!(landed.current.as_deref(), Some("high"));
    assert_eq!(landed.pending, None);
}

#[test]
fn a_press_mid_turn_or_with_no_legend_types_no_arrow_after_the_command_and_closes_what_it_opened() {
    let stood = StoppedWorker::stand_with(
        98_300,
        "--agent claude --model claude-opus-5-5 --effort medium",
        "",
        Vec::new(),
    );
    let term = stood.host.worker_term;
    let ran = a_progressed_turn("read it", "medium");
    std::fs::write(&stood.host.transcript, format!("{}\n", ran.join("\n"))).unwrap();
    crate::orchestration::pane_turn_began(term, stood.began + 4_000);
    let tail = tail_of(&stood);
    assert_eq!(
        press(&stood.host, term, tail.as_deref(), "high"),
        Err(PressRefused::Stuck(Stuck::TurnRunning))
    );
    assert!(stood.sent_at_worker().is_empty(), "nothing typed mid-turn");

    let owned: Vec<&str> = ran.iter().map(String::as_str).collect();
    stood.stops_on(&owned, stood.began + 6_000);
    // No legend ever shows: the command went in, no arrow follows it, and
    // one Esc closes whatever it opened.
    assert_eq!(
        press(&stood.host, term, tail.as_deref(), "high"),
        Err(PressRefused::Door("picker_missing"))
    );
    assert_eq!(
        stood.sent_at_worker(),
        [CLAUDE_EFFORT_PICKER.open, "\r", CLAUDE_EFFORT_PICKER.cancel]
    );
    let chip = pane_effort(&stood.host, term, tail.as_deref());
    assert_eq!(chip.pending, None, "a refused press moved nothing");
}

#[test]
fn a_codex_pane_is_shown_and_never_pressed() {
    let stood = StoppedWorker::stand_with(
        98_400,
        "--agent codex --model gpt-6-astra --effort xhigh",
        "",
        Vec::new(),
    );
    let term = stood.host.worker_term;
    stood.stops_on(&[], stood.began + 5_000);
    let chip = pane_effort(&stood.host, term, None);
    assert_eq!(chip.current.as_deref(), Some("xhigh"));
    assert_eq!(chip.stuck, Some(Stuck::OnScreen));
    assert_eq!(chip.door, Some("shown"));
    assert_eq!(chip.command, Some("/model"));
    assert_eq!(
        press(&stood.host, term, None, "high"),
        Err(PressRefused::Stuck(Stuck::OnScreen))
    );
    assert!(stood.sent_at_worker().is_empty());
}

/// The step-effort seat under `on` would raise a stuck turn's effort; on a
/// pane a person moved it neither asks nor types.
#[test]
fn the_step_effort_seat_leaves_a_pane_the_person_moved_alone() {
    use zerocode_core::jev::{JevMode, STEP_EFFORT};
    let stood = StoppedWorker::stand_with(
        98_500,
        "--agent claude --model claude-opus-5-5 --effort medium",
        "",
        Vec::new(),
    );
    let term = stood.host.worker_term;
    let raise =
        crate::systemone::tests::Endpoint::serving("HTTP/1.1 200 OK", a_move_answer("raise"), 0);
    let home = stood.asks_jev_for(&STEP_EFFORT, &raise, JevMode::On, "/wt");
    let began = stood.began + 10_000;
    let ran = a_progressed_turn("read it", "medium");
    let owned: Vec<&str> = ran.iter().map(String::as_str).collect();
    stood.stops_on(&owned, began);
    picker_up(&stood);
    press(&stood.host, term, tail_of(&stood).as_deref(), "low").expect("the press");
    *stood.host.screen.lock().unwrap() = String::new();
    let typed = stood.sent_at_worker().len();

    let mut stuck = ran.clone();
    stuck.extend(a_stuck_turn("fix the build", "low"));
    a_turn_runs_then_ends(&stood, &stuck, began + 1_000, began + 2_000);
    for beat in [3_000, 4_000, 5_000, 6_000] {
        tick(&stood.host, &[], began + beat);
    }
    assert!(
        StoppedWorker::rows_of(&home, &STEP_EFFORT).is_empty(),
        "the seat judged a pane the person moved"
    );
    assert!(raise.asked().is_empty(), "the seat asked about it");
    assert_eq!(stood.sent_at_worker().len(), typed, "the seat typed at it");
}

/// Two presses overlap: while the first waits for the picker's legend, a
/// second is refused before it types — a second `/effort⏎` landing on an open
/// picker would press its "Enter to confirm" and save the account's default.
#[test]
fn a_second_press_while_the_first_is_typing_is_refused_before_any_key() {
    let stood = StoppedWorker::stand_with(
        98_600,
        "--agent claude --model claude-opus-5-5 --effort medium",
        "",
        Vec::new(),
    );
    let term = stood.host.worker_term;
    let ran = a_progressed_turn("read it", "medium");
    let owned: Vec<&str> = ran.iter().map(String::as_str).collect();
    stood.stops_on(&owned, stood.began + 5_000);
    let tail = tail_of(&stood);
    std::thread::scope(|scope| {
        let first = scope.spawn(|| press(&stood.host, term, tail.as_deref(), "max"));
        let since = std::time::Instant::now();
        while stood.sent_at_worker().len() < 2 {
            assert!(
                since.elapsed() < std::time::Duration::from_secs(5),
                "the first press never typed its command"
            );
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(
            pane_effort(&stood.host, term, tail.as_deref()).stuck,
            Some(Stuck::PressMoving),
            "the chip offers a press while one is typing"
        );
        assert_eq!(
            press(&stood.host, term, tail.as_deref(), "low"),
            Err(PressRefused::Stuck(Stuck::PressMoving))
        );
        assert_eq!(
            stood.sent_at_worker(),
            [CLAUDE_EFFORT_PICKER.open, "\r"],
            "the second press typed at a pane the first is moving"
        );
        picker_up(&stood);
        let first = first.join().expect("the first press").expect("lands");
        assert_eq!(first.pending.as_deref(), Some("max"));
        assert_eq!(first.stuck, None, "the pane is free once the press is done");
    });
}

/// The session's record said another rung than the press put it on: the
/// record wins, the chip drops the pressed word, and the next press counts
/// its arrows from the rung the record names.
#[test]
fn a_pressed_word_the_next_turn_contradicts_is_dropped_and_the_next_press_counts_from_the_record() {
    let stood = StoppedWorker::stand_with(
        98_700,
        "--agent claude --model claude-opus-5-5 --effort medium",
        "",
        Vec::new(),
    );
    let term = stood.host.worker_term;
    let ran = a_progressed_turn("read it", "high");
    let owned: Vec<&str> = ran.iter().map(String::as_str).collect();
    stood.stops_on(&owned, stood.began + 5_000);
    picker_up(&stood);
    let chip = press(&stood.host, term, tail_of(&stood).as_deref(), "max").expect("the press");
    assert_eq!(chip.pending.as_deref(), Some("max"));

    let mut next = ran.clone();
    next.extend(a_progressed_turn("go on", "xhigh"));
    let owned: Vec<&str> = next.iter().map(String::as_str).collect();
    stood.stops_on(&owned, stood.began + 9_000);
    let tail = tail_of(&stood);
    let read = pane_effort(&stood.host, term, tail.as_deref());
    assert_eq!(read.current.as_deref(), Some("xhigh"));
    assert_eq!(
        read.pending, None,
        "the chip kept a word the record contradicts"
    );

    let typed = stood.sent_at_worker().len();
    press(&stood.host, term, tail.as_deref(), "high").expect("the next press");
    assert_eq!(
        &stood.sent_at_worker()[typed..],
        [
            CLAUDE_EFFORT_PICKER.open,
            "\r",
            CLAUDE_EFFORT_PICKER.lower,
            CLAUDE_EFFORT_PICKER.session_only,
        ],
        "one arrow down from the recorded xhigh, not two from the pressed max"
    );
}

/// The step-effort seat under `shadow` judged a turn and typed nothing: its
/// move waits for the next turn to label it, and a person's press between
/// the two turns goes through.
#[test]
fn a_press_goes_through_while_a_recording_seat_waits_to_label_its_move() {
    use zerocode_core::jev::{JevMode, STEP_EFFORT};
    let stood = StoppedWorker::stand_with(
        98_800,
        "--agent claude --model claude-opus-5-5 --effort medium",
        "",
        Vec::new(),
    );
    let term = stood.host.worker_term;
    let raise =
        crate::systemone::tests::Endpoint::serving("HTTP/1.1 200 OK", a_move_answer("raise"), 0);
    let home = stood.asks_jev_for(&STEP_EFFORT, &raise, JevMode::Shadow, "/wt");
    let began = stood.began + 10_000;
    let stuck = a_stuck_turn("fix the build", "medium");
    a_turn_runs_then_ends(&stood, &stuck, began, began + 1_000);
    for beat in [2_000, 3_000, 4_000, 5_000] {
        tick(&stood.host, &[], began + beat);
    }
    assert_eq!(
        StoppedWorker::rows_of(&home, &STEP_EFFORT).len(),
        1,
        "the seat judged the turn"
    );
    assert!(
        stood.sent_at_worker().is_empty(),
        "a recording seat types nothing"
    );
    let tail = tail_of(&stood);
    assert_eq!(pane_effort(&stood.host, term, tail.as_deref()).stuck, None);
    picker_up(&stood);
    let chip = press(&stood.host, term, tail.as_deref(), "high").expect("the press");
    assert_eq!(chip.pending.as_deref(), Some("high"));
    assert_eq!(
        stood.sent_at_worker(),
        [
            CLAUDE_EFFORT_PICKER.open,
            "\r",
            CLAUDE_EFFORT_PICKER.raise,
            CLAUDE_EFFORT_PICKER.session_only,
        ]
    );
}

/// The seat under `on` keyed its raise and waits for the next turn to label
/// it: a person's press between the two turns goes through, and counts its
/// arrows from the rung the seat's keys put the session on.
#[test]
fn a_press_after_the_seat_keyed_its_move_counts_from_the_rung_the_seat_put_the_session_on() {
    use zerocode_core::jev::{JevMode, STEP_EFFORT};
    let stood = StoppedWorker::stand_with(
        98_900,
        "--agent claude --model claude-opus-5-5 --effort medium",
        "",
        Vec::new(),
    );
    let term = stood.host.worker_term;
    let raise =
        crate::systemone::tests::Endpoint::serving("HTTP/1.1 200 OK", a_move_answer("raise"), 0);
    let _home = stood.asks_jev_for(&STEP_EFFORT, &raise, JevMode::On, "/wt");
    let began = stood.began + 10_000;
    let stuck = a_stuck_turn("fix the build", "medium");
    a_turn_runs_then_ends(&stood, &stuck, began, began + 1_000);
    tick(&stood.host, &[], began + 2_000);
    tick(&stood.host, &[], began + 3_000);
    picker_up(&stood);
    tick(&stood.host, &[], began + 4_000);
    tick(&stood.host, &[], began + 5_000);
    assert_eq!(
        stood.sent_at_worker(),
        [
            CLAUDE_EFFORT_PICKER.open,
            "\r",
            CLAUDE_EFFORT_PICKER.raise,
            CLAUDE_EFFORT_PICKER.session_only,
        ],
        "the seat keyed medium → high"
    );
    let tail = tail_of(&stood);
    let chip = pane_effort(&stood.host, term, tail.as_deref());
    assert_eq!(chip.stuck, None, "a settled move holds the pane");
    assert_eq!(chip.pending.as_deref(), Some("high"));
    press(&stood.host, term, tail.as_deref(), "max").expect("the press");
    assert_eq!(
        &stood.sent_at_worker()[4..],
        [
            CLAUDE_EFFORT_PICKER.open,
            "\r",
            CLAUDE_EFFORT_PICKER.raise,
            CLAUDE_EFFORT_PICKER.raise,
            CLAUDE_EFFORT_PICKER.session_only,
        ],
        "two arrows up from the seat's high, not three from the recorded medium"
    );
}

/// A pane the person opened themselves — no worker sits in it, the window's
/// own launch record says it runs claude (t-81861, t-81532 r1 P3-6).
fn persons_pane(stood: &StoppedWorker, term: u32) -> u32 {
    *stood.host.persons.lock().unwrap() = Some(term);
    term
}

/// t-81861 (P3-6): the person's own pane, the road the review named and no
/// Rust test walked — the chip shows the session's record, a press moves it
/// through the picker for this session only, the keys land at that pane.
#[test]
fn a_pane_the_person_opened_shows_the_record_and_takes_a_press() {
    let stood = StoppedWorker::stand_with(
        98_950,
        "--agent claude --model claude-opus-5-5 --effort medium",
        "",
        Vec::new(),
    );
    let term = persons_pane(&stood, stood.host.worker_term + 40);
    let ran = a_progressed_turn("read it", "medium");
    std::fs::write(&stood.host.transcript, format!("{}\n", ran.join("\n"))).unwrap();
    super::pane_turn_ended(term, stood.began + 4_999, false, stood.began + 5_000);
    picker_up(&stood);
    let tail = tail_of(&stood);
    let chip = pane_effort(&stood.host, term, tail.as_deref());
    assert_eq!(chip.current.as_deref(), Some("medium"));
    assert_eq!(chip.witness, EffortWitness::Turn, "no summons: the record");
    assert_eq!(chip.stuck, None);
    let chip = press(&stood.host, term, tail.as_deref(), "high").expect("the press");
    assert_eq!(
        stood.host.typed_at(term),
        [
            CLAUDE_EFFORT_PICKER.open,
            "\r",
            CLAUDE_EFFORT_PICKER.raise,
            CLAUDE_EFFORT_PICKER.session_only,
        ]
    );
    assert_eq!(chip.pending.as_deref(), Some("high"));
}

/// t-81861 (P3-2): a turn the person stopped with Esc is the person's own
/// pane's best moment to lower the effort; the refusal 「지난 턴을 직접 멈춘
/// 판」 guards what the window types unasked, and a press is asked. A worker's
/// pane the ledger summoned keeps the refusal.
#[test]
fn a_person_who_stopped_their_own_turn_can_press_and_a_summoned_workers_pane_stays_held() {
    let stood = StoppedWorker::stand_with(
        98_960,
        "--agent claude --model claude-opus-5-5 --effort medium",
        "",
        Vec::new(),
    );
    let term = persons_pane(&stood, stood.host.worker_term + 40);
    let ran = a_progressed_turn("read it", "medium");
    std::fs::write(&stood.host.transcript, format!("{}\n", ran.join("\n"))).unwrap();
    super::pane_turn_ended(term, stood.began + 4_999, true, stood.began + 5_000);
    super::pane_turn_ended(
        stood.host.worker_term,
        stood.began + 4_999,
        true,
        stood.began + 5_000,
    );
    let tail = tail_of(&stood);
    let own = pane_effort(&stood.host, term, tail.as_deref());
    assert_eq!(
        own.stuck, None,
        "the person's own pane, stopped by hand: {own:?}"
    );
    let worker = pane_effort(&stood.host, stood.host.worker_term, tail.as_deref());
    assert_eq!(worker.stuck, Some(Stuck::HeldByPerson), "{worker:?}");
}

/// t-81861 (P2-2, the half r2 left): a move the seat keyed whose label window
/// ran out before any turn ended is still the rung the picker stands on — the
/// record says the rung before, and a press counts from where the session is
/// until a turn has run.
#[test]
fn a_rung_the_seat_keyed_stays_the_rung_after_its_label_window_until_a_turn_runs() {
    use zerocode_core::jev::{JevMode, STEP_EFFORT};
    use zerocode_core::step_effort::STEP_EFFORT_LABEL_WINDOW_MS;
    let stood = StoppedWorker::stand_with(
        98_970,
        "--agent claude --model claude-opus-5-5 --effort medium",
        "",
        Vec::new(),
    );
    let term = stood.host.worker_term;
    let raise =
        crate::systemone::tests::Endpoint::serving("HTTP/1.1 200 OK", a_move_answer("raise"), 0);
    let _home = stood.asks_jev_for(&STEP_EFFORT, &raise, JevMode::On, "/wt");
    let began = stood.began + 10_000;
    let stuck = a_stuck_turn("fix the build", "medium");
    a_turn_runs_then_ends(&stood, &stuck, began, began + 1_000);
    tick(&stood.host, &[], began + 2_000);
    tick(&stood.host, &[], began + 3_000);
    picker_up(&stood);
    tick(&stood.host, &[], began + 4_000);
    tick(&stood.host, &[], began + 5_000);
    // No turn ends; the label window runs out and the seat writes its row.
    tick(
        &stood.host,
        &[],
        began + 5_000 + STEP_EFFORT_LABEL_WINDOW_MS + 1_000,
    );
    let tail = tail_of(&stood);
    let chip = pane_effort(&stood.host, term, tail.as_deref());
    assert_eq!(
        chip.pending.as_deref(),
        Some("high"),
        "the picker stands on the rung the seat keyed: {chip:?}"
    );
    let sent = stood.sent_at_worker().len();
    press(&stood.host, term, tail.as_deref(), "max").expect("the press");
    assert_eq!(
        &stood.sent_at_worker()[sent..],
        [
            CLAUDE_EFFORT_PICKER.open,
            "\r",
            CLAUDE_EFFORT_PICKER.raise,
            CLAUDE_EFFORT_PICKER.raise,
            CLAUDE_EFFORT_PICKER.session_only,
        ],
        "two arrows from the keyed high, not three from the recorded medium"
    );
    // A turn that runs afterwards says where the session stands.
    let mut next = stuck.clone();
    next.extend(a_progressed_turn("go on", "max"));
    let owned: Vec<&str> = next.iter().map(String::as_str).collect();
    stood.stops_on(&owned, began + 9_000_000);
    let landed = pane_effort(&stood.host, term, tail_of(&stood).as_deref());
    assert_eq!(landed.current.as_deref(), Some("max"));
    assert_eq!(landed.pending, None);
}

/// t-81861 (picker_missing, the seat's side): a picker command that opened
/// without its legend is closed with the picker's own cancel key at a pane
/// between turns, as a person's press closes it — no picker stands open that
/// nobody drives.
#[test]
fn the_seat_closes_a_picker_it_opened_but_never_saw_the_legend_of() {
    use zerocode_core::jev::{JevMode, STEP_EFFORT};
    let stood = StoppedWorker::stand_with(
        98_980,
        "--agent claude --model claude-opus-5-5 --effort medium",
        "",
        Vec::new(),
    );
    let raise =
        crate::systemone::tests::Endpoint::serving("HTTP/1.1 200 OK", a_move_answer("raise"), 0);
    let _home = stood.asks_jev_for(&STEP_EFFORT, &raise, JevMode::On, "/wt");
    let began = stood.began + 10_000;
    let stuck = a_stuck_turn("fix the build", "medium");
    a_turn_runs_then_ends(&stood, &stuck, began, began + 1_000);
    tick(&stood.host, &[], began + 2_000);
    tick(&stood.host, &[], began + 3_000);
    // The command is in; the screen never shows the legend.
    tick(&stood.host, &[], began + 4_000);
    tick(
        &stood.host,
        &[],
        began + 4_000 + crate::orchestration::step_effort::PICKER_WAIT_MS + 1_000,
    );
    assert_eq!(
        stood.sent_at_worker(),
        [CLAUDE_EFFORT_PICKER.open, "\r", CLAUDE_EFFORT_PICKER.cancel],
        "the command, then Esc: no arrow into a picker nobody saw"
    );
}

/// The seat's stuck-turn road up to its picker command being typed, the
/// screen never showing the legend; `then` runs with the stand and the
/// moment the legend's wait runs out (t-85293 r2).
fn a_seat_waiting_on_a_legend_that_never_shows(
    term_base: u32,
    then: impl FnOnce(&StoppedWorker, i64),
) {
    use zerocode_core::jev::{JevMode, STEP_EFFORT};
    let stood = StoppedWorker::stand_with(
        term_base,
        "--agent claude --model claude-opus-5-5 --effort medium",
        "",
        Vec::new(),
    );
    let raise =
        crate::systemone::tests::Endpoint::serving("HTTP/1.1 200 OK", a_move_answer("raise"), 0);
    let _home = stood.asks_jev_for(&STEP_EFFORT, &raise, JevMode::On, "/wt");
    let began = stood.began + 10_000;
    let stuck = a_stuck_turn("fix the build", "medium");
    a_turn_runs_then_ends(&stood, &stuck, began, began + 1_000);
    tick(&stood.host, &[], began + 2_000);
    tick(&stood.host, &[], began + 3_000);
    tick(&stood.host, &[], began + 4_000);
    assert_eq!(
        stood.sent_at_worker(),
        [CLAUDE_EFFORT_PICKER.open, "\r"],
        "the command is in and nothing else"
    );
    let out = began + 4_000 + crate::orchestration::step_effort::PICKER_WAIT_MS + 1_000;
    then(&stood, out);
}

/// t-85293 r2 (P3-1): Esc is safe only between turns — an Esc into a turn
/// stops it. A turn that began while the seat waited for the legend gets no
/// Esc; delete `look.at_rest` from the close and this fails.
#[test]
fn the_seat_types_no_esc_into_a_turn_that_began_while_it_waited_for_the_legend() {
    a_seat_waiting_on_a_legend_that_never_shows(98_990, |stood, out| {
        crate::orchestration::pane_turn_began(stood.host.worker_term, out - 500);
        tick(&stood.host, &[], out);
        assert_eq!(
            stood.sent_at_worker(),
            [CLAUDE_EFFORT_PICKER.open, "\r"],
            "the command and its Enter, no Esc into the running turn"
        );
    });
}

/// t-85293 r2 (P3-4): the person's own words on the line are theirs; an Esc
/// there may clear them. The seat closes a legend-less picker only over an
/// empty line, as every door that types unasked leaves a draft alone.
#[test]
fn the_seat_types_no_esc_over_the_persons_words_on_the_line() {
    a_seat_waiting_on_a_legend_that_never_shows(99_000, |stood, out| {
        stood
            .host
            .draft
            .store(true, std::sync::atomic::Ordering::SeqCst);
        tick(&stood.host, &[], out);
        assert_eq!(
            stood.sent_at_worker(),
            [CLAUDE_EFFORT_PICKER.open, "\r"],
            "a draft stands: no Esc"
        );
    });
}

/// A press on the worker pane, a short moment after which `then` happens
/// while the press waits for a legend that never shows. Returns what the pane
/// was sent and what the press answered.
fn a_press_that_waits_for_a_legend_while(
    term_base: u32,
    then: impl FnOnce(&StoppedWorker) + Send,
) -> (Vec<String>, Result<EffortChip, PressRefused>) {
    let stood = StoppedWorker::stand_with(
        term_base,
        "--agent claude --model claude-opus-5-5 --effort medium",
        "",
        Vec::new(),
    );
    let term = stood.host.worker_term;
    let ran = a_progressed_turn("read it", "medium");
    let owned: Vec<&str> = ran.iter().map(String::as_str).collect();
    stood.stops_on(&owned, stood.began + 5_000);
    let tail = tail_of(&stood);
    let answer = std::thread::scope(|scope| {
        scope.spawn(|| {
            std::thread::sleep(std::time::Duration::from_millis(400));
            then(&stood);
        });
        press(&stood.host, term, tail.as_deref(), "high")
    });
    (stood.sent_at_worker(), answer)
}

/// t-85293 r2 (P3-1, the press's side): a turn that began while the press
/// waited gets no Esc; delete `composer_at_rest` from `carry` and this fails.
#[test]
fn a_press_types_no_esc_into_a_turn_that_began_while_it_waited_for_the_legend() {
    let (sent, answer) = a_press_that_waits_for_a_legend_while(99_010, |stood| {
        crate::orchestration::pane_turn_began(stood.host.worker_term, stood.began + 6_000);
    });
    assert_eq!(answer.err(), Some(PressRefused::Door("picker_missing")));
    assert_eq!(
        sent,
        [CLAUDE_EFFORT_PICKER.open, "\r"],
        "no Esc into a turn"
    );
}

/// t-85293 r2 (P3-4, the press's side): the person's words on the line are
/// not cleared by the Esc that closes a legend-less picker.
#[test]
fn a_press_types_no_esc_over_the_persons_words_on_the_line() {
    let (sent, answer) = a_press_that_waits_for_a_legend_while(99_020, |stood| {
        stood
            .host
            .draft
            .store(true, std::sync::atomic::Ordering::SeqCst);
    });
    assert_eq!(answer.err(), Some(PressRefused::Door("picker_missing")));
    assert_eq!(sent, [CLAUDE_EFFORT_PICKER.open, "\r"], "a draft stands");
}

/// t-85293 r2 (P3-2): the person's own pane, its turn stopped by hand, is
/// theirs to press on (t-81861 P3-2) — so the Esc that closes a picker that
/// never showed its legend is theirs too. Without it the half-open menu
/// stays on their screen.
#[test]
fn a_press_on_the_persons_own_stopped_pane_closes_the_picker_it_opened_without_a_legend() {
    let stood = StoppedWorker::stand_with(
        99_030,
        "--agent claude --model claude-opus-5-5 --effort medium",
        "",
        Vec::new(),
    );
    let term = persons_pane(&stood, stood.host.worker_term + 40);
    let ran = a_progressed_turn("read it", "medium");
    std::fs::write(&stood.host.transcript, format!("{}\n", ran.join("\n"))).unwrap();
    super::pane_turn_ended(term, stood.began + 4_999, true, stood.began + 5_000);
    let tail = tail_of(&stood);
    assert_eq!(
        press(&stood.host, term, tail.as_deref(), "high").err(),
        Some(PressRefused::Door("picker_missing"))
    );
    assert_eq!(
        stood.host.typed_at(term),
        [CLAUDE_EFFORT_PICKER.open, "\r", CLAUDE_EFFORT_PICKER.cancel],
        "the command, then Esc: the person stopped the turn, the menu is closed for them"
    );
}

/// t-85293 r2 (P3-3): a turn stopped by hand leaves the agent when the
/// person quits it; a press then would type `/effort` into the shell. The
/// shell check is made on that road too, as on the road of a turn that ended
/// by itself.
#[test]
fn a_press_on_the_persons_stopped_pane_with_a_shell_in_front_types_nothing() {
    let stood = StoppedWorker::stand_with(
        99_040,
        "--agent claude --model claude-opus-5-5 --effort medium",
        "",
        Vec::new(),
    );
    let term = persons_pane(&stood, stood.host.worker_term + 40);
    let ran = a_progressed_turn("read it", "medium");
    std::fs::write(&stood.host.transcript, format!("{}\n", ran.join("\n"))).unwrap();
    super::pane_turn_ended(term, stood.began + 4_999, true, stood.began + 5_000);
    stood
        .host
        .shell
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let tail = tail_of(&stood);
    let chip = pane_effort(&stood.host, term, tail.as_deref());
    assert_eq!(chip.stuck, Some(Stuck::ShellInFront), "{chip:?}");
    assert_eq!(
        press(&stood.host, term, tail.as_deref(), "high").err(),
        Some(PressRefused::Stuck(Stuck::ShellInFront))
    );
    assert!(
        stood.host.typed_at(term).is_empty(),
        "nothing went to the shell"
    );
}
