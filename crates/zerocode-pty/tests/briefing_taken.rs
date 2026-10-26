//! A launch briefing that a booting CLI threw away is noticed (t-74381).
//!
//! Each fake CLI is a shell script in a real pty that wears one agent's shape:
//! it shakes hands, draws its composer glyph at once, and then — like Claude
//! Code 2.1.281 on its way up — reads and discards everything for a while
//! before its real composer stands. The driver below is the launch road in
//! small: the delivery the window runs, then the watch over what it delivered.

#![cfg(unix)]

use std::time::{Duration, Instant};

use zerocode_pty::briefing::{BriefingWatch, WatchStep, briefing_shows};
use zerocode_pty::{DeliveryOutcome, DeliveryStep, Observed, PromptDelivery, PtyLane, ReadySignal};

/// What the driver found out.
struct Journey {
    ended: WatchStep,
    pasted: usize,
    screen: String,
}

/// One agent's shape: its composer glyph, its bytes in octal for `printf`, and
/// what it prints once it has taken a line.
struct Shape {
    name: &'static str,
    glyph: char,
    glyph_octal: &'static str,
    working: &'static str,
}

const CLAUDE: Shape = Shape {
    name: "claude",
    glyph: '\u{276f}',
    glyph_octal: "\\342\\235\\257",
    working: "\\342\\217\\272 working  esc to interrupt",
};

const CODEX: Shape = Shape {
    name: "codex",
    glyph: '\u{203a}',
    glyph_octal: "\\342\\200\\272",
    working: "Working (0s \\342\\200\\242 esc to interrupt)",
};

const BRIEFING: &str = "You are a worker in this window's orchestration, carrying task T-STAMP-7";

/// The script of a CLI that draws its glyph right after the handshake and
/// then, for `boot` seconds, reads and throws away what it is typed — or for
/// ever when `boot` is `forever` — before a composer that keeps what it is
/// given. (A background job in a script has no terminal on its input, so the
/// discarding `dd` is handed the pty by name.)
fn fake_cli(shape: &Shape, boot: &str) -> String {
    let glyph = shape.glyph_octal;
    let working = shape.working;
    let discard = if boot == "forever" {
        "dd of=/dev/null bs=1 2>/dev/null".to_string()
    } else if boot == "0" {
        // A CLI that is up already throws nothing away.
        ":".to_string()
    } else {
        format!(
            "dd of=/dev/null bs=1 </dev/tty 2>/dev/null & D=$!; sleep {boot}; kill $D 2>/dev/null; wait $D 2>/dev/null"
        )
    };
    format!(
        "stty raw -echo; printf '\\033[?2004h'; sleep 0.2; printf '{glyph} '; {discard}; \
         printf '\\033[?2004l'; sleep 0.1; printf '\\033[?2004h\\r\\n{glyph} '; \
         stty cooked -echo; \
         while IFS= read -r line; do printf '\\r\\n{glyph} %s\\r\\n{working}\\r\\n' \"$line\"; done"
    )
}

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

/// The launch road in small: a delivery to the glyph, then the watch, and a
/// second delivery when the watch says to type again.
fn launch(shape: &Shape, boot: &str) -> Journey {
    let mut lane = PtyLane::spawn(
        "/bin/sh",
        &["-c".to_string(), fake_cli(shape, boot)],
        None,
        &[],
        24,
        100,
    )
    .expect("spawn");
    let window = Duration::from_millis(3_500);
    let mut delivery = Some(PromptDelivery::with_deadlines(
        BRIEFING.to_string(),
        true,
        ReadySignal::Prompt(shape.glyph),
        Instant::now(),
        Duration::from_millis(300),
        Duration::from_secs(10),
    ));
    let mut watch: Option<BriefingWatch> = None;
    let mut pasted = 0;
    let give_up = Instant::now() + Duration::from_secs(40);
    let ended = loop {
        assert!(
            Instant::now() < give_up,
            "the journey never ended: {:?}",
            all_text(&mut lane)
        );
        let pumped = lane.pump();
        if let Some(running) = delivery.as_mut() {
            let listening_for = running.marker();
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
            match running.poll(seen, Instant::now()) {
                DeliveryStep::Waiting => {}
                DeliveryStep::Write(bytes) | DeliveryStep::Submit(bytes) => {
                    if bytes
                        .windows(BRIEFING.len())
                        .any(|slice| slice == BRIEFING.as_bytes())
                    {
                        pasted += 1;
                    }
                    lane.write_input(&bytes).expect("write to child");
                }
                DeliveryStep::Done(DeliveryOutcome::Delivered) => {
                    delivery = None;
                    watch.get_or_insert_with(|| BriefingWatch::new(Instant::now(), window));
                }
                DeliveryStep::Done(outcome) => panic!("the delivery ended {outcome:?}"),
            }
        } else if let Some(watching) = watch.as_mut() {
            let shown = briefing_shows(&all_text(&mut lane), BRIEFING);
            match watching.poll(shown, Instant::now()) {
                WatchStep::Waiting => {}
                WatchStep::Retype => {
                    // The composer is up by now: any sign of rest says go.
                    delivery = Some(PromptDelivery::with_deadlines(
                        BRIEFING.to_string(),
                        true,
                        ReadySignal::Rest(Some(shape.glyph)),
                        Instant::now(),
                        Duration::from_millis(300),
                        Duration::from_secs(10),
                    ));
                }
                end @ (WatchStep::Taken | WatchStep::Undelivered) => break end,
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    let screen = all_text(&mut lane);
    Journey {
        ended,
        pasted,
        screen,
    }
}

/// The loss itself: the first briefing goes into a CLI that is still booting
/// and is thrown away. Today's launch road calls that delivered and moves on;
/// the watch sees an empty composer, types the briefing again once the real
/// composer stands, and the pane then holds it exactly once.
#[test]
fn a_briefing_the_booting_cli_threw_away_is_typed_again_and_then_taken() {
    for shape in [&CLAUDE, &CODEX] {
        let journey = launch(shape, "1.5");
        assert_eq!(
            journey.ended,
            WatchStep::Taken,
            "{}: {:?}",
            shape.name,
            journey.screen
        );
        assert_eq!(
            journey.pasted, 2,
            "{}: the briefing was typed {} time(s)",
            shape.name, journey.pasted
        );
        assert_eq!(
            journey.screen.matches("T-STAMP-7").count(),
            1,
            "{}: the briefing stands once on the screen: {:?}",
            shape.name,
            journey.screen
        );
    }
}

/// A CLI that never takes input is not a success with a quiet pane: after the
/// one retype the watch says the briefing is undelivered.
#[test]
fn a_cli_that_never_takes_its_briefing_is_reported_undelivered_after_one_retype() {
    for shape in [&CLAUDE, &CODEX] {
        let journey = launch(shape, "forever");
        assert_eq!(
            journey.ended,
            WatchStep::Undelivered,
            "{}: {:?}",
            shape.name,
            journey.screen
        );
        assert_eq!(
            journey.pasted, 2,
            "{}: typed {} time(s), not twice",
            shape.name, journey.pasted
        );
    }
}

/// And a CLI that took the briefing the first time is left alone: it is not
/// typed at twice.
#[test]
fn a_briefing_that_was_taken_the_first_time_is_not_typed_again() {
    for shape in [&CLAUDE, &CODEX] {
        let journey = launch(shape, "0");
        assert_eq!(
            journey.ended,
            WatchStep::Taken,
            "{}: {:?}",
            shape.name,
            journey.screen
        );
        assert_eq!(
            journey.pasted, 1,
            "{}: typed {} time(s)",
            shape.name, journey.pasted
        );
    }
}
