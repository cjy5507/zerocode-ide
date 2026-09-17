//! Prompt delivery against a real child process.
//!
//! The unit tests prove the state machine's answers; this proves the whole
//! journey — a real pty, a child that turns bracketed paste on partway
//! through its life, and the driver writing into it. What is asserted is what
//! a person would care about: nothing is typed at the child before it says it
//! reads keys, the hostile parts of a prompt arrive defused, and the Enter
//! actually submits the line.

#![cfg(unix)]

use std::time::{Duration, Instant};

use zerocode_pty::ready::{Guard, Line, Refusal};
use zerocode_pty::{DeliveryOutcome, DeliveryStep, Observed, PromptDelivery, PtyLane, ReadySignal};

/// The composer glyph the child below prints (`›`, the printf's octal
/// escapes). Which agent wears which glyph is its catalog row's fact.
const COMPOSER_PROMPT: char = '\u{203a}';

/// Everything the child has put on the screen, scrollback included.
fn all_text(lane: &mut PtyLane) -> String {
    let grid = lane.terminal_mut().grid_mut();
    let mut text = String::new();
    for index in 0..grid.scrollback_len() {
        text.push_str(&grid.scrollback_line(index));
        text.push('\n');
    }
    text.push_str(&grid.visible_text());
    text
}

/// A prompt reaches a real child defused, gated, and submitted.
///
/// The child sleeps first, so the early rounds are a program that has NOT
/// shaken hands — a write during them would be the "typed at a shell" bug.
/// Then it enables bracketed paste and turns into `cat`, the simplest program
/// that proves submission: in canonical mode the kernel hands `cat` the line
/// only when Enter arrives, so the text appearing a second time (the echo
/// being the first) is the Enter having landed.
#[test]
fn a_prompt_reaches_a_real_child_defused_and_submitted() {
    let mut lane = PtyLane::spawn(
        "/bin/sh",
        &[
            "-c".to_string(),
            // The handshake arrives mid-life, not at spawn.
            "sleep 0.3; printf '\\033[?2004h'; cat".to_string(),
        ],
        None,
        &[],
        24,
        80,
    )
    .expect("spawn");

    // The hostile part: an escape that, un-defused, would close the paste
    // envelope and clear the child's screen as if typed.
    let prompt = "evil\x1b[2J stamp-A";
    let mut delivery = PromptDelivery::with_deadlines(
        prompt.to_string(),
        true,
        // Quiet: `cat` has no prompt glyph and never touches its cursor, so
        // silence is the only signal it will ever give.
        ReadySignal::Quiet,
        Instant::now(),
        Duration::from_millis(300),
        Duration::from_secs(10),
    );

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut wrote_before_handshake = false;
    let mut delivered = false;
    while Instant::now() < deadline {
        let pumped = lane.pump();
        let seen = {
            let grid = lane.terminal_mut().grid_mut();
            Observed {
                wrote: pumped.bytes > 0,
                bracketed_paste: grid.bracketed_paste(),
                cursor_shows: grid.cursor_shows(),
                ..Observed::default()
            }
        };
        let shaken = seen.bracketed_paste;
        match delivery.poll(seen, Instant::now()) {
            DeliveryStep::Waiting => {}
            DeliveryStep::Write(bytes) | DeliveryStep::Submit(bytes) => {
                if !shaken {
                    wrote_before_handshake = true;
                }
                lane.write_input(&bytes).expect("write to child");
            }
            DeliveryStep::Done(DeliveryOutcome::Delivered) => {
                delivered = true;
                break;
            }
            DeliveryStep::Done(outcome) => {
                panic!("the delivery ended {outcome:?} against a child that did shake hands");
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(delivered, "the delivery never finished");
    assert!(
        !wrote_before_handshake,
        "bytes were written before the child said it reads keys"
    );

    // Now the proof on the child's own screen. Two appearances of the stamp:
    // the tty echo as the paste went in, and `cat`'s copy — which canonical
    // mode only releases when the Enter arrives.
    let saw = |lane: &mut PtyLane, want: &str, count: usize| {
        let until = Instant::now() + Duration::from_secs(5);
        while Instant::now() < until {
            lane.pump();
            if all_text(lane).matches(want).count() >= count {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    };
    assert!(
        saw(&mut lane, "stamp-A", 2),
        "the Enter never submitted the line: screen was {:?}",
        all_text(&mut lane)
    );
    // The escape arrived as the visible symbol, not as a control byte — the
    // screen still holds everything, so nothing executed a clear.
    assert!(
        saw(&mut lane, "␛[2J", 1),
        "the hostile escape was not defused on its way in: {:?}",
        all_text(&mut lane)
    );
    assert!(
        all_text(&mut lane).contains("evil"),
        "the screen was cleared — the escape executed after all"
    );
}

/// A composer glyph ends the wait the moment the child draws it.
///
/// The whole point of the glyph signal: an agent that announces its input line
/// by printing `›` on it has no other tell, so without this the delivery sits
/// through the entire eight-second timeout and then declines to send at all —
/// waiting on silence is not even an option, because a program that is drawing
/// is not silent.
///
/// The clock fed to the delivery is synthetic and moves a millisecond per
/// round while the real child takes its own time, so nothing here can be
/// finished by a deadline: a paste at all is the glyph having been seen. The
/// observation is assembled exactly as the pump assembles it, marker included
/// — that seam is the thing being proved.
#[test]
fn a_composer_glyph_ends_the_wait_the_moment_it_is_drawn() {
    let mut lane = PtyLane::spawn(
        "/bin/sh",
        &[
            "-c".to_string(),
            // Handshake, a pause, then the composer line — the glyph has to
            // arrive after the handshake round to count for anything. `cat`
            // afterwards so the paste has somewhere to land.
            "printf '\\033[?2004h'; sleep 0.3; printf '\\342\\200\\272 '; cat".to_string(),
        ],
        None,
        &[],
        24,
        80,
    )
    .expect("spawn");

    let start = Instant::now();
    let mut delivery = PromptDelivery::new(
        "stamp-C".to_string(),
        true,
        ReadySignal::Prompt(COMPOSER_PROMPT),
        start,
    );

    let mut clock = start;
    let real_deadline = Instant::now() + Duration::from_secs(10);
    let mut pasted_at: Option<Duration> = None;
    let mut delivered = false;
    while Instant::now() < real_deadline {
        let pumped = lane.pump();
        // Exactly the pump's assembly: ask the grid about the glyph this
        // delivery is listening for, every round, and take the round with it.
        let listening_for = delivery.marker();
        let seen = {
            let grid = lane.terminal_mut().grid_mut();
            let drawn = grid.take_glyph_drawn(listening_for);
            Observed {
                wrote: pumped.bytes > 0,
                bracketed_paste: grid.bracketed_paste(),
                cursor_shows: grid.cursor_shows(),
                marker_written: drawn.anywhere,
                marker_in_alt: drawn.in_alt_screen,
                alt_screen: grid.alt_screen(),
            }
        };
        clock += Duration::from_millis(1);
        match delivery.poll(seen, clock) {
            DeliveryStep::Waiting => {}
            DeliveryStep::Write(bytes) | DeliveryStep::Submit(bytes) => {
                pasted_at.get_or_insert_with(|| clock.duration_since(start));
                lane.write_input(&bytes).expect("write to child");
            }
            DeliveryStep::Done(DeliveryOutcome::Delivered) => {
                delivered = true;
                break;
            }
            DeliveryStep::Done(outcome) => {
                panic!("the glyph was drawn and the delivery ended {outcome:?}: {clock:?}")
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    assert!(delivered, "the delivery never finished");
    let waited = pasted_at.expect("nothing was ever pasted");
    // A prompt signal has one deadline and it is the whole budget. Landing an
    // order of magnitude inside it is the difference between "the agent said
    // so" and "we gave up and typed anyway".
    assert!(
        waited < zerocode_pty::ready::TIMEOUT / 8,
        "the paste waited {waited:?} of the {:?} budget — that is a fallback, \
         not a glyph",
        zerocode_pty::ready::TIMEOUT
    );

    // And it really landed: the echo, then `cat`'s copy once the Enter freed
    // the line.
    let until = Instant::now() + Duration::from_secs(5);
    let mut saw = false;
    while Instant::now() < until {
        lane.pump();
        if all_text(&mut lane).matches("stamp-C").count() >= 2 {
            saw = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        saw,
        "the prompt never reached the child: screen was {:?}",
        all_text(&mut lane)
    );
}

/// A child that never shakes hands is never typed at.
///
/// `cat` alone: perfectly alive, visibly quiet, cursor shown — everything a
/// naive readiness check would fall for — and it never enables bracketed
/// paste. The delivery must end as a timeout with nothing written.
#[test]
fn a_child_that_never_shakes_hands_is_never_typed_at() {
    let mut lane = PtyLane::spawn(
        "/bin/sh",
        &["-c".to_string(), "cat".to_string()],
        None,
        &[],
        24,
        80,
    )
    .expect("spawn");

    let mut delivery = PromptDelivery::with_deadlines(
        "never send this".to_string(),
        true,
        ReadySignal::Quiet,
        Instant::now(),
        Duration::from_millis(100),
        Duration::from_millis(600),
    );

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        assert!(Instant::now() < deadline, "the timeout never fired");
        let pumped = lane.pump();
        let seen = {
            let grid = lane.terminal_mut().grid_mut();
            Observed {
                wrote: pumped.bytes > 0,
                bracketed_paste: grid.bracketed_paste(),
                cursor_shows: grid.cursor_shows(),
                ..Observed::default()
            }
        };
        match delivery.poll(seen, Instant::now()) {
            DeliveryStep::Waiting => std::thread::sleep(Duration::from_millis(10)),
            DeliveryStep::Write(_) | DeliveryStep::Submit(_) => {
                panic!("a prompt was typed at a plain cat")
            }
            DeliveryStep::Done(outcome) => {
                assert_eq!(outcome, DeliveryOutcome::TimedOut);
                break;
            }
        }
    }
    assert!(!delivery.pasted());
    // And the child's screen never heard about it.
    lane.pump();
    assert!(
        !all_text(&mut lane).contains("never send this"),
        "the prompt reached the child anyway"
    );
}

/// A hand between the paste and the Enter, against a real child: the words
/// land and the Enter does not.
///
/// The same `cat` proof as the first test, read the other way round. In
/// canonical mode `cat` only receives the line when Enter arrives, so a stamp
/// that appears ONCE — the tty echo — and never twice is a line that was
/// pasted and left unsubmitted. The line facts are the pump's own seam: the
/// driver reports a hand the round after the paste, exactly as the window
/// does when a person types while the words settle, and the outcome says how
/// far the delivery got.
#[test]
fn a_hand_after_the_paste_leaves_the_line_unsubmitted_in_a_real_child() {
    let mut lane = PtyLane::spawn(
        "/bin/sh",
        &["-c".to_string(), "printf '\\033[?2004h'; cat".to_string()],
        None,
        &[],
        24,
        80,
    )
    .expect("spawn");

    let mut delivery = PromptDelivery::with_deadlines(
        "stamp-D".to_string(),
        true,
        ReadySignal::Quiet,
        Instant::now(),
        Duration::from_millis(300),
        Duration::from_secs(10),
    )
    .guarded(Guard::for_somebody_elses_line(None));

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut wrote = Vec::new();
    let mut outcome = None;
    let mut line = Line {
        hand: Some(3),
        ..Line::default()
    };
    while Instant::now() < deadline {
        let pumped = lane.pump();
        let seen = {
            let grid = lane.terminal_mut().grid_mut();
            Observed {
                wrote: pumped.bytes > 0,
                bracketed_paste: grid.bracketed_paste(),
                cursor_shows: grid.cursor_shows(),
                ..Observed::default()
            }
        };
        match delivery.poll_line(seen, line, Instant::now()) {
            DeliveryStep::Waiting => {}
            DeliveryStep::Write(bytes) => {
                lane.write_input(&bytes).expect("write to child");
                wrote.extend_from_slice(&bytes);
                // The person reaches the line while the words settle.
                line = Line {
                    draft: true,
                    hand: Some(4),
                    ..line
                };
            }
            DeliveryStep::Submit(_) => panic!("an Enter followed a hand"),
            DeliveryStep::Done(ended) => {
                outcome = Some(ended);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(
        outcome,
        Some(DeliveryOutcome::Unsubmitted(Refusal::HandReached))
    );
    assert!(delivery.pasted());
    assert!(
        !wrote.contains(&b'\r'),
        "a carriage return reached the child: {wrote:?}"
    );

    // The echo is on screen; `cat`'s copy never is, because no Enter freed
    // the line. Waited out rather than asserted at once, so a slow child
    // cannot pass by not having drawn the echo yet.
    let until = Instant::now() + Duration::from_secs(3);
    let mut echoed = false;
    while Instant::now() < until {
        lane.pump();
        if all_text(&mut lane).contains("stamp-D") {
            echoed = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(
        echoed,
        "the paste never reached the child: {:?}",
        all_text(&mut lane)
    );
    std::thread::sleep(Duration::from_millis(300));
    lane.pump();
    assert_eq!(
        all_text(&mut lane).matches("stamp-D").count(),
        1,
        "the line was submitted after all: {:?}",
        all_text(&mut lane)
    );
}
