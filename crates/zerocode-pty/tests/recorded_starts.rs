//! Readiness against programs as they really started.
//!
//! The unit tests prove the machine's answers to observations somebody wrote
//! down by hand; these hand it observations a real program made. Each fixture
//! is a pty capture of a fresh start — every read with its arrival time — and
//! the replay turns a delivery against it the way the pump does, so what is
//! asserted is the one thing that decides whether words land: which bytes had
//! arrived when the delivery wrote.
//!
//! Claude Code is why these exist (t-4530, 2026-09-17). From 2.1.274 on — and
//! as the recordings show, 2.1.273 as well, only faster — it turns bracketed
//! paste on within a third of a second, stays silent while it starts, then
//! takes that input handler down (`?2004l`) and mounts the composer it draws
//! `❯` on under a new one. A wait that reads silence as readiness pastes into
//! the handler about to go whenever the silence outlasts the quiet window:
//! 2.1.274's three silent seconds always do.

use std::time::Duration;

use zerocode_pty::recording::{Recording, Replayed};
use zerocode_pty::{DeliveryOutcome, PromptDelivery, ReadySignal};

/// The glyph Claude Code draws on its composer line in every recording here.
/// The window's copy of the fact is the claude row in the agent catalog.
const CLAUDE_CODE_COMPOSER: char = '\u{276f}';

/// One pump round per millisecond — finer than the window's pump, so a write
/// goes in the round its deciding bytes arrived and an order is never an
/// artefact of a round boundary. The window's own cadence is replayed end to
/// end by the shell's test on the claude row.
const ROUND: Duration = Duration::from_millis(1);

const STARTS: [(&str, &str); 4] = [
    (
        "2.1.273 fullscreen",
        include_str!("fixtures/claude-code-2.1.273-fullscreen.reads"),
    ),
    (
        "2.1.274 fullscreen",
        include_str!("fixtures/claude-code-2.1.274-fullscreen.reads"),
    ),
    (
        "2.1.273 inline",
        include_str!("fixtures/claude-code-2.1.273-inline.reads"),
    ),
    (
        "2.1.274 inline",
        include_str!("fixtures/claude-code-2.1.274-inline.reads"),
    ),
];

fn start(name: &str) -> Recording {
    let (_, text) = STARTS
        .iter()
        .find(|(start, _)| *start == name)
        .unwrap_or_else(|| panic!("no recording called {name}"));
    Recording::parse(text).unwrap_or_else(|bad| panic!("{name}: {bad}"))
}

fn briefing(recording: &Recording, signal: &ReadySignal) -> Replayed {
    recording.replay(ROUND, Duration::ZERO, |now| {
        PromptDelivery::new("You are a worker".to_string(), true, signal.clone(), now)
    })
}

/// Every recorded start takes its first input handler down before its
/// composer stands — the shape the readiness door has to survive.
#[test]
fn every_recorded_claude_code_start_takes_its_first_input_handler_down() {
    for (name, _) in STARTS {
        let replayed = briefing(&start(name), &ReadySignal::Prompt(CLAUDE_CODE_COMPOSER));
        assert!(
            !replayed.handlers_down.is_empty(),
            "{name}: no input handler came down"
        );
    }
}

/// Silence pastes 2.1.274's briefing into the handler it then takes down.
///
/// This is the measured loss, pinned: the quiet window opens 1.5 s after the
/// first handshake, inside a silence that lasts three seconds, and the paste
/// lands on a screen with no composer on it. Live, such words never appear
/// (probe 2026-09-17: a paste 1.8 s into a 2.1.274 start was never seen,
/// while pastes right after the `❯` — four starts, both versions, both
/// screen modes — showed in the composer within 131 ms).
#[test]
fn a_quiet_wait_pastes_claude_code_2_1_274s_briefing_into_a_handler_it_takes_down() {
    let replayed = briefing(&start("2.1.274 fullscreen"), &ReadySignal::Quiet);
    let paste = replayed
        .pastes()
        .next()
        .expect("silence came and the paste went");
    assert!(
        paste.at < replayed.handlers_down[0],
        "the quiet paste at {:?} no longer lands before the teardown at {:?}",
        paste.at,
        replayed.handlers_down
    );
    assert!(
        !paste.screen.contains(CLAUDE_CODE_COMPOSER),
        "a composer was on screen at the quiet paste:\n{}",
        paste.screen
    );
}

/// The composer's glyph opens the door only under the handler that stays,
/// in both screen modes and both versions — and nothing goes before it.
#[test]
fn a_composer_glyph_wait_pastes_only_once_the_composer_stands() {
    for (name, _) in STARTS {
        let replayed = briefing(&start(name), &ReadySignal::Prompt(CLAUDE_CODE_COMPOSER));
        assert_eq!(
            replayed.outcome,
            Some(DeliveryOutcome::Delivered),
            "{name}: {replayed:?}"
        );
        assert_eq!(
            replayed.pastes().count(),
            1,
            "{name}: {:?}",
            replayed.writes
        );
        for write in &replayed.writes {
            assert!(
                replayed.handlers_down.iter().all(|down| *down < write.at),
                "{name}: a write at {:?} went into a handler taken down at {:?}",
                write.at,
                replayed.handlers_down
            );
            assert!(
                write.bracketed_paste && write.screen.contains(CLAUDE_CODE_COMPOSER),
                "{name}: a write at {:?} went before the composer stood:\n{}",
                write.at,
                write.screen
            );
        }
    }
}

/// An idle composer does not announce itself twice — why a send to one that
/// is already up waits for rest, never for the launch glyph.
///
/// 2.1.274 drew its `❯` once, 3.4 s into the start. In the idle that follows
/// it wrote twice — a usage notice at 13.4 s and its erasure at 21.4 s — and
/// drew no glyph, so a launch wait begun at the standing composer starves
/// while a rest wait begun at the same moment delivers.
#[test]
fn an_idle_claude_code_composer_never_draws_its_glyph_again() {
    let recording = start("2.1.274 fullscreen");
    let mounted = briefing(&recording, &ReadySignal::Prompt(CLAUDE_CODE_COMPOSER));
    let stood = mounted.pastes().next().expect("the composer stood").at;
    let begun_at_the_composer = |signal: ReadySignal| {
        recording.replay(ROUND, stood, |now| {
            PromptDelivery::new("next".to_string(), true, signal, now)
        })
    };
    let launch = begun_at_the_composer(ReadySignal::Prompt(CLAUDE_CODE_COMPOSER));
    assert!(
        launch.writes.is_empty(),
        "an idle composer drew its glyph again: {:?}",
        launch.writes
    );
    assert_eq!(launch.outcome, Some(DeliveryOutcome::TimedOut));
    let rest = begun_at_the_composer(ReadySignal::Rest(Some(CLAUDE_CODE_COMPOSER)));
    assert_eq!(rest.outcome, Some(DeliveryOutcome::Delivered), "{rest:?}");
}
