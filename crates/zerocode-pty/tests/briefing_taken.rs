//! A launch briefing that a booting CLI threw away is noticed (t-74381), and
//! the briefing typed again counts only once its own delivery says it was
//! submitted (t-74919).
//!
//! Each fake CLI is a shell script in a real pty that wears one agent's shape:
//! it shakes hands, draws its composer glyph at once, and then — like Claude
//! Code 2.1.281 on its way up — reads and discards everything for a while
//! before its real composer stands, which echoes what it is given. The launch
//! road is the window's in small and in the window's order: deliveries run on
//! a pump thread of their own while [`confirm`] — the function the window's
//! launch road calls — reads the screen and types again on this one.

#![cfg(unix)]

use std::sync::mpsc::{Receiver, SyncSender, sync_channel};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use zerocode_pty::briefing::{BriefingPane, Unheard, Verdict, confirm};
use zerocode_pty::ready::{Guard, Line, Refusal};
use zerocode_pty::{DeliveryOutcome, DeliveryStep, Observed, PromptDelivery, PtyLane, ReadySignal};

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

/// The launch the pane holds, as the window fingerprints it; a relaunch under
/// the briefing typed again makes it the next one.
const LAUNCH: u64 = 1;

/// How long a delivery waits for its pane's readiness here.
const DELIVERY_DEADLINE: Duration = Duration::from_secs(10);

/// The script of a CLI that draws its glyph right after the handshake and
/// then, for `boot` seconds, reads and throws away what it is typed — or for
/// ever when `boot` is `forever` — then takes its handshake down for `gap`
/// seconds before a composer that echoes and keeps what it is given. (A
/// background job in a script has no terminal on its input, so the
/// discarding `dd` is handed the pty by name.)
fn fake_cli(shape: &Shape, boot: &str, gap: &str) -> String {
    let glyph = shape.glyph_octal;
    let working = shape.working;
    let reads = format!("while IFS= read -r line; do printf '{working}\\r\\n'; done");
    if boot == "0" {
        // A CLI that is up already throws nothing away: its first glyph is
        // its composer's, which echoes from the start.
        return format!(
            "stty cooked echo; printf '\\033[?2004h'; sleep 0.2; printf '{glyph} '; {reads}"
        );
    }
    let discard = if boot == "forever" {
        "dd of=/dev/null bs=1 2>/dev/null".to_string()
    } else {
        format!(
            "dd of=/dev/null bs=1 </dev/tty 2>/dev/null & D=$!; sleep {boot}; kill $D 2>/dev/null; wait $D 2>/dev/null"
        )
    };
    format!(
        "stty raw -echo; printf '\\033[?2004h'; sleep 0.2; printf '{glyph} '; {discard}; \
         printf '\\033[?2004l'; sleep {gap}; printf '\\033[?2004h\\r\\n{glyph} '; \\
         stty cooked echo; {reads}"
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

/// What the pump thread and the watch share: the pane, the delivery on it and
/// who waits for that delivery's outcome — the window's `deliveries` and
/// `delivery_waiters` in small.
struct Pane {
    lane: PtyLane,
    delivery: Option<(PromptDelivery, SyncSender<DeliveryOutcome>)>,
    /// Whether the pane is relaunched between the paste and the Enter of the
    /// briefing typed again.
    relaunch_under_retype: bool,
    /// The launch the line holds now.
    launch: u64,
    /// How many times the briefing was written to the pane.
    pasted: usize,
    /// When the briefing was typed again, and when that delivery ended how.
    retyped_at: Option<Instant>,
    retype_ended: Option<(Instant, DeliveryOutcome)>,
    stop: bool,
}

/// The pump: one round every 10 ms, the delivery fed the line as it stands.
fn pump(shared: Arc<Mutex<Pane>>, starts_after: Duration) -> JoinHandle<()> {
    std::thread::spawn(move || {
        std::thread::sleep(starts_after);
        loop {
            {
                let mut pane = shared.lock().expect("pane");
                if pane.stop {
                    return;
                }
                let pumped = pane.lane.pump();
                if let Some((mut delivery, notify)) = pane.delivery.take() {
                    let seen = {
                        let grid = pane.lane.terminal_mut().grid_mut();
                        let drawn = grid.take_glyph_drawn(delivery.marker());
                        Observed {
                            wrote: pumped.bytes > 0,
                            bracketed_paste: grid.bracketed_paste(),
                            cursor_shows: grid.cursor_shows(),
                            marker_written: drawn.anywhere,
                            marker_in_alt: drawn.in_alt_screen,
                            alt_screen: grid.alt_screen(),
                        }
                    };
                    let line = Line {
                        launch: Some(pane.launch),
                        ..Line::default()
                    };
                    match delivery.poll_line(seen, line, Instant::now()) {
                        DeliveryStep::Waiting => pane.delivery = Some((delivery, notify)),
                        DeliveryStep::Write(bytes) | DeliveryStep::Submit(bytes) => {
                            if bytes
                                .windows(BRIEFING.len())
                                .any(|slice| slice == BRIEFING.as_bytes())
                            {
                                pane.pasted += 1;
                                if pane.retyped_at.is_some() && pane.relaunch_under_retype {
                                    pane.launch = LAUNCH + 1;
                                }
                            }
                            pane.lane.write_input(&bytes).expect("write to child");
                            pane.delivery = Some((delivery, notify));
                        }
                        DeliveryStep::Done(outcome) => {
                            if pane.retyped_at.is_some() {
                                pane.retype_ended = Some((Instant::now(), outcome));
                            }
                            let _ = notify.send(outcome);
                        }
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    })
}

/// A delivery of the briefing to `signal`, guarded for the pane's own launch
/// as the window's are.
fn delivery(signal: ReadySignal) -> PromptDelivery {
    PromptDelivery::with_deadlines(
        BRIEFING.to_string(),
        true,
        signal,
        Instant::now(),
        Duration::from_millis(300),
        DELIVERY_DEADLINE,
    )
    .clearing(false)
    .guarded(Guard::for_its_own_line(Some(LAUNCH)))
}

/// The watch's hands on the shared pane.
struct Watched {
    shared: Arc<Mutex<Pane>>,
    glyph: char,
    retype_registers_after: Duration,
}

impl BriefingPane for Watched {
    fn screen(&mut self) -> Option<String> {
        Some(all_text(&mut self.shared.lock().expect("pane").lane))
    }

    fn retype(&mut self) -> Receiver<DeliveryOutcome> {
        std::thread::sleep(self.retype_registers_after);
        let (notify, waiting) = sync_channel(1);
        let mut pane = self.shared.lock().expect("pane");
        pane.retyped_at = Some(Instant::now());
        // The composer is up by now: any sign of rest says go.
        pane.delivery = Some((delivery(ReadySignal::Rest(Some(self.glyph))), notify));
        waiting
    }

    fn withdraw(&mut self) {
        self.shared.lock().expect("pane").delivery = None;
    }
}

/// A machine under load, made on purpose: how late the pump thread starts and
/// how late the watch's retype is registered. A test that holds on a loaded
/// machine holds on these.
#[derive(Clone, Copy)]
struct Load {
    pump_starts_after: Duration,
    retype_registers_after: Duration,
}

impl Load {
    const NONE: Load = Load {
        pump_starts_after: Duration::ZERO,
        retype_registers_after: Duration::ZERO,
    };
}

/// What the launch found out.
struct Journey {
    verdict: Verdict,
    pasted: usize,
    screen: String,
    /// How long the briefing typed again took to end, and how it ended.
    retype: Option<(Duration, DeliveryOutcome)>,
}

/// The launch road in small: the first delivery to the glyph, awaited, then
/// the watch over what it delivered while the pump keeps running.
fn launch(shape: &Shape, boot: &str, gap: &str, window: Duration, relaunch: bool) -> Journey {
    launch_under(shape, boot, gap, window, relaunch, Load::NONE)
}

fn launch_under(
    shape: &Shape,
    boot: &str,
    gap: &str,
    window: Duration,
    relaunch: bool,
    load: Load,
) -> Journey {
    let lane = PtyLane::spawn(
        "/bin/sh",
        &["-c".to_string(), fake_cli(shape, boot, gap)],
        None,
        &[],
        24,
        100,
    )
    .expect("spawn");
    let (notify, first) = sync_channel(1);
    let shared = Arc::new(Mutex::new(Pane {
        lane,
        delivery: Some((delivery(ReadySignal::Prompt(shape.glyph)), notify)),
        relaunch_under_retype: relaunch,
        launch: LAUNCH,
        pasted: 0,
        retyped_at: None,
        retype_ended: None,
        stop: false,
    }));
    let pumping = pump(Arc::clone(&shared), load.pump_starts_after);
    let outcome = first
        .recv_timeout(DELIVERY_DEADLINE * 2)
        .expect("the first delivery ended");
    assert_eq!(outcome, DeliveryOutcome::Delivered, "{}", shape.name);
    let mut watched = Watched {
        shared: Arc::clone(&shared),
        glyph: shape.glyph,
        retype_registers_after: load.retype_registers_after,
    };
    let verdict = confirm(&mut watched, BRIEFING, window, DELIVERY_DEADLINE * 2);
    let mut pane = shared.lock().expect("pane");
    pane.stop = true;
    let journey = Journey {
        verdict,
        pasted: pane.pasted,
        screen: all_text(&mut pane.lane),
        retype: pane
            .retyped_at
            .zip(pane.retype_ended)
            .map(|(at, (ended, how))| (ended.duration_since(at), how)),
    };
    drop(pane);
    pumping.join().expect("pump");
    journey
}

/// The loss itself: the first briefing goes into a CLI that is still booting
/// and is thrown away. The watch sees an empty composer, types the briefing
/// again once the real composer stands, and the pane then holds it exactly
/// once.
#[test]
fn a_briefing_the_booting_cli_threw_away_is_typed_again_and_then_taken() {
    for shape in [&CLAUDE, &CODEX] {
        let journey = launch(shape, "1.5", "0.1", Duration::from_millis(3_500), false);
        assert_eq!(
            journey.verdict,
            Verdict::Taken,
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
        let journey = launch(shape, "forever", "0.1", Duration::from_millis(3_500), false);
        assert_eq!(
            journey.verdict,
            Verdict::Undelivered(Unheard::NeverShown),
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
        let journey = launch(shape, "0", "0.1", Duration::from_millis(3_500), false);
        assert_eq!(
            journey.verdict,
            Verdict::Taken,
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

/// R2 8 (t-74919): the briefing typed again lands in the composer, and the
/// pane is relaunched before its Enter, so the delivery withholds the Enter.
/// The words stand on the screen all the same — and a launch that read only
/// the screen called that a success. The retype's own outcome is the
/// verdict: no start.
#[test]
fn a_retyped_briefing_whose_enter_was_withheld_is_no_start_though_the_screen_shows_it() {
    for shape in [&CLAUDE, &CODEX] {
        let journey = launch(shape, "1.5", "0.1", Duration::from_millis(3_500), true);
        assert_eq!(
            journey.verdict,
            Verdict::Undelivered(Unheard::RetypeEnded(DeliveryOutcome::Unsubmitted(
                Refusal::LaunchChanged
            ))),
            "{}: retype {:?}, screen {:?}",
            shape.name,
            journey.retype,
            journey.screen
        );
        assert!(
            journey.screen.contains("T-STAMP-7"),
            "{}: the withheld words were never on the screen, so this is not the race: {:?}",
            shape.name,
            journey.screen
        );
    }
}

/// R2 9 (t-74919): the CLI's composer comes back later after the retype than
/// one watch window — but well inside the delivery's own deadline. The watch
/// waits for that delivery instead of giving up on its own clock, and the
/// briefing is taken.
#[test]
fn a_retype_that_outlasts_the_watch_window_but_not_its_deadline_is_waited_for() {
    let window = Duration::from_secs(2);
    for shape in [&CLAUDE, &CODEX] {
        let journey = launch(shape, "1.5", "5", window, false);
        assert_eq!(
            journey.verdict,
            Verdict::Taken,
            "{}: retype {:?}, screen {:?}",
            shape.name,
            journey.retype,
            journey.screen
        );
        let (took, how) = journey.retype.expect("the briefing was typed again");
        assert_eq!(how, DeliveryOutcome::Delivered, "{}", shape.name);
        assert!(
            took > window,
            "{}: the retype ended in {took:?}, inside one window — not the case under test",
            shape.name
        );
        assert_eq!(journey.pasted, 2, "{}", shape.name);
    }
}

/// R3 (t-75231): the machine is slow to start pumping the pane — three
/// seconds, past the 1.5 s the fake CLI boots for — and the first briefing
/// is still thrown away, because the CLI's boot ends when the test says so
/// and not when a clock does. The assertions are the loss test's own.
#[test]
fn a_pump_that_starts_late_still_loses_the_first_briefing_and_takes_the_second() {
    let load = Load {
        pump_starts_after: Duration::from_secs(3),
        ..Load::NONE
    };
    for shape in [&CLAUDE, &CODEX] {
        let journey = launch_under(
            shape,
            "1.5",
            "0.1",
            Duration::from_millis(3_500),
            false,
            load,
        );
        assert_eq!(
            journey.verdict,
            Verdict::Taken,
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

/// R3 (t-75231): the retype is registered four seconds after the watch asks
/// for it — most of the old five-second gap gone — and the composer still
/// comes back after one watch window of the retype's own life, because the
/// test lets it come back only then.
#[test]
fn a_retype_registered_late_still_outlasts_the_watch_window() {
    let window = Duration::from_secs(2);
    let load = Load {
        retype_registers_after: Duration::from_secs(4),
        ..Load::NONE
    };
    for shape in [&CLAUDE, &CODEX] {
        let journey = launch_under(shape, "1.5", "5", window, false, load);
        assert_eq!(
            journey.verdict,
            Verdict::Taken,
            "{}: retype {:?}, screen {:?}",
            shape.name,
            journey.retype,
            journey.screen
        );
        let (took, how) = journey.retype.expect("the briefing was typed again");
        assert_eq!(how, DeliveryOutcome::Delivered, "{}", shape.name);
        assert!(
            took > window,
            "{}: the retype ended in {took:?}, inside one window — not the case under test",
            shape.name
        );
        assert_eq!(journey.pasted, 2, "{}", shape.name);
    }
}
