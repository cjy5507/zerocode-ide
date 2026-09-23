//! Main-thread ping state machine; rides the existing system-resume thread.
//!
//! The observer testifies only for time it was there to watch (t-6388). Its
//! judgment runs on an uptime clock, and uptime keeps running while macOS
//! gives this process no CPU at all — a lid-closed DarkWake, a sleep being
//! entered or left. Of the 27 hangs of 2026-09-17..24 the power log could
//! place, 25 fell in such a stretch with no screen awake; the main thread
//! had not been busy, it had not been scheduled. So a beat that
//! finds its own nap a whole beat late, or finds the wall clock run ahead of
//! uptime since the beat before (a sleep, wherever this thread was), or finds
//! no screen awake, withdraws the episode and warms up again.

use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{
    AtomicBool, AtomicU64,
    Ordering::{AcqRel, Acquire, Relaxed, Release},
};
use std::time::{Duration, Instant};
use tauri::{Emitter as _, Manager as _};

use crate::crash::Limits;

static READY: AtomicBool = AtomicBool::new(false);
static ENABLED: AtomicBool = AtomicBool::new(true);
static FIRST_MS: AtomicU64 = AtomicU64::new(Limits::DEFAULT.first_ms);
static SECOND_MS: AtomicU64 = AtomicU64::new(Limits::DEFAULT.second_ms);

pub(crate) fn configure(limits: Limits) {
    ENABLED.store(limits.watchdog, Relaxed);
    FIRST_MS.store(limits.first_ms, Relaxed);
    SECOND_MS.store(limits.second_ms, Relaxed);
}

pub(crate) fn ready() {
    READY.store(true, Relaxed);
}

#[derive(Default, Debug, PartialEq, Eq)]
struct Tick {
    ping: bool,
    hang: Option<u64>,
    badge: Option<u64>,
    /// A judgment that had crossed the first threshold, withdrawn because
    /// its witness was not there for it.
    voided: Option<Voided>,
    /// How long a hang first seen still unanswered really lasted, said on
    /// the beat its answer arrives. The hang line's own number is only when
    /// it was seen, which for such a hang is always the first threshold.
    ended: Option<u64>,
}

/// One beat of the observer, as its own clocks saw it.
#[derive(Clone, Copy, Debug)]
struct Beat {
    /// Uptime ms since the monitor started — the clock every judgment uses.
    now: u64,
    /// Wall-clock ms at the same moment. It runs on through a system sleep
    /// that uptime skips, so between two beats the two part by the sleep.
    wall: u64,
    /// How long the nap before this beat really took; one beat was asked.
    nap: u64,
    /// Whether any screen is awake to show the window.
    seen: bool,
}

/// Why the observer cannot testify for the time just past.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Absence {
    /// Its own nap came back this many ms past the beat it asked for: the
    /// process was not being run, the main thread included.
    Late(u64),
    /// The wall clock ran this many ms ahead of uptime since the last beat:
    /// the machine slept, wherever this thread happened to be.
    Slept(u64),
    /// No screen was awake: a lid-closed DarkWake, or a display asleep.
    Unseen,
}

impl fmt::Display for Absence {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Late(ms) => write!(f, "observer_late_ms={ms}"),
            Self::Slept(ms) => write!(f, "slept_ms={ms}"),
            Self::Unseen => f.write_str("no_screen_awake"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Voided {
    ms: u64,
    why: Absence,
}

struct Watch {
    warm_until: u64,
    pending: Option<u64>,
    noted: bool,
    badged: bool,
    count: u64,
    /// The previous beat's `(uptime, wall)` — the evidence of a sleep.
    last: Option<(u64, u64)>,
}

impl Watch {
    fn new(now: u64) -> Self {
        Self {
            warm_until: now + Limits::DEFAULT.warmup_ms,
            pending: None,
            noted: false,
            badged: false,
            count: 0,
            last: None,
        }
    }

    /// Whether the observer was there for the time since its last beat.
    fn absence(&mut self, beat: Beat, limits: Limits) -> Option<Absence> {
        let previous = self.last.replace((beat.now, beat.wall));
        let late = beat.nap.saturating_sub(limits.ping_ms);
        if late >= limits.late_ms() {
            return Some(Absence::Late(late));
        }
        let slept = previous.map_or(0, |(now, wall)| {
            beat.wall
                .saturating_sub(wall)
                .saturating_sub(beat.now.saturating_sub(now))
        });
        if slept >= limits.late_ms() {
            return Some(Absence::Slept(slept));
        }
        (!beat.seen).then_some(Absence::Unseen)
    }

    fn step(&mut self, beat: Beat, answered: Option<u64>, limits: Limits) -> Tick {
        let mut tick = Tick::default();
        let now = beat.now;
        let absence = self.absence(beat, limits);
        if !limits.watchdog {
            self.warm_until = now.saturating_add(limits.warmup_ms);
            self.pending = None;
            return tick;
        }
        if let Some(why) = absence {
            // A witness cannot testify for time it was not there for, nor
            // for a window nobody could see: the episode is withdrawn and
            // the watch warms up again, as it does after a boot or a sleep.
            if let Some(sent) = self.pending.take() {
                let ms = answered.unwrap_or(now).saturating_sub(sent);
                if !self.noted && ms >= limits.first_ms {
                    tick.voided = Some(Voided { ms, why });
                }
            }
            self.warm_until = now.saturating_add(limits.warmup_ms);
            self.noted = false;
            self.badged = false;
            return tick;
        }
        if now < self.warm_until {
            return tick;
        }
        if let Some(sent) = self.pending {
            // Judge when the main thread answered, not when THIS thread was
            // next scheduled. A busy diagnostics thread cannot frame a healthy UI.
            let ms = answered.unwrap_or(now).saturating_sub(sent);
            let seen_open = self.noted;
            if !self.noted && ms >= limits.first_ms {
                self.noted = true;
                self.count += 1;
                tick.hang = Some(ms);
            }
            if !self.badged && ms >= limits.second_ms {
                self.badged = true;
                tick.badge = Some(ms);
            }
            if answered.is_none() {
                return tick;
            }
            if seen_open {
                tick.ended = Some(ms);
            }
        }
        self.pending = Some(now);
        self.noted = false;
        self.badged = false;
        tick.ping = true;
        tick
    }
}

struct Answer {
    outstanding: AtomicBool,
    at_ms: AtomicU64,
}

pub(crate) struct Monitor {
    start: Instant,
    watch: Watch,
    answer: Arc<Answer>,
    armed: bool,
}

impl Monitor {
    pub(crate) fn new() -> Self {
        Self {
            start: Instant::now(),
            watch: Watch::new(0),
            answer: Arc::new(Answer {
                outstanding: AtomicBool::new(false),
                at_ms: AtomicU64::new(0),
            }),
            armed: false,
        }
    }
    pub(crate) fn resumed(&mut self) {
        self.armed = false;
    }
    /// One beat, after a nap that really took `nap` of wall time.
    pub(crate) fn tick(&mut self, app: &tauri::AppHandle, nap: Duration) {
        let now = self.start.elapsed().as_millis() as u64;
        if !READY.load(Relaxed) {
            return;
        }
        if !self.armed {
            let count = self.watch.count;
            self.watch = Watch::new(now);
            self.watch.count = count;
            self.armed = true;
        }
        let first_ms = FIRST_MS.load(Relaxed);
        let limits = Limits {
            watchdog: ENABLED.load(Relaxed),
            first_ms,
            second_ms: SECOND_MS
                .load(Relaxed)
                .max(first_ms + Limits::DEFAULT.ping_ms),
            ..Limits::DEFAULT
        };
        let answered = (!self.answer.outstanding.load(Acquire))
            .then(|| self.answer.at_ms.swap(0, Relaxed))
            .filter(|at| *at != 0);
        let beat = Beat {
            now,
            wall: wall_ms(),
            nap: u64::try_from(nap.as_millis()).unwrap_or(u64::MAX),
            seen: screens_awake(),
        };
        let tick = self.watch.step(beat, answered, limits);
        if let Some(Voided { ms, why }) = tick.voided {
            // Said once per withdrawn judgment, so a week of these lines
            // counts what the old watchdog would have filed as hangs.
            crate::crumbs::record("hang_withdrawn", format_args!("ms={ms} {why}"));
            crate::note_window_event(
                app.state::<crate::AppState>().local_data_root(),
                &format!("hang withdrawn: {ms}ms {why}"),
            );
        }
        if let Some(ms) = tick.ended {
            // The hang's real length, the number a week of these is read by.
            crate::crumbs::record("hang_ended", format_args!("ms={ms}"));
            crate::note_window_event(
                app.state::<crate::AppState>().local_data_root(),
                &format!("hang ended: {ms}ms"),
            );
        }
        if tick.ping {
            // A disabled/resumed monitor can abandon a judgment, but it cannot
            // cancel a queued Tauri closure. Keep that queue slot until it answers.
            if self
                .answer
                .outstanding
                .compare_exchange(false, true, AcqRel, Acquire)
                .is_ok()
            {
                let answer = Arc::clone(&self.answer);
                let start = self.start;
                if app
                    .run_on_main_thread(move || {
                        answer
                            .at_ms
                            .store(start.elapsed().as_millis() as u64, Relaxed);
                        answer.outstanding.store(false, Release);
                    })
                    .is_err()
                {
                    self.answer.outstanding.store(false, Release);
                    self.armed = false;
                }
            } else {
                self.watch.pending = None;
            }
        }
        // A census rides its own main-thread closure, queued behind the ping so
        // it never lengthens the answer it exists to explain, and only when due.
        if tick.ping {
            let now_wall = wall_ms();
            if crate::view_census::due(crate::view_census::last_at_ms(), now_wall) {
                let census_app = app.clone();
                let _ = app.run_on_main_thread(move || {
                    let census =
                        tauri::Manager::get_webview_window(&census_app, crate::MAIN_WINDOW_LABEL)
                            .and_then(|window| crate::view_census::take(&window));
                    if let Some(census) = census {
                        crate::view_census::note(wall_ms(), census);
                    }
                });
            }
        }
        if let Some(ms) = tick.hang {
            let scope = if answered.is_none() {
                crate::crumbs::main_scope()
            } else {
                "answered_before_observation".to_string()
            };
            let command = crate::crumbs::last_command();
            // The last view census, so a mouse-move stack (AppKit re-collects
            // every tracking area per move) comes with its N.
            let census = crate::view_census::last_summary(wall_ms())
                .map_or_else(String::new, |census| format!(" {census}"));
            // Capture before sampling: recovery during the sample is possible,
            // and this scope is the observation at the threshold, not afterwards.
            crate::crumbs::record(
                "hang",
                format_args!("ms={ms} main_scope={scope} last_command={command}{census}"),
            );
            let state = app.state::<crate::AppState>();
            crate::note_window_event(
                state.local_data_root(),
                &format!("hang: {ms}ms main_scope={scope} last_command={command}{census}"),
            );
            let sample = if answered.is_none() {
                crate::hang_sample::sample_main_thread()
            } else {
                Err("answered_before_observation")
            };
            let frames = match sample {
                Ok(frames) => {
                    for frame in &frames {
                        crate::crumbs::record(
                            "main_sample_after_detection",
                            format_args!("{frame}"),
                        );
                    }
                    frames.join("\n")
                }
                Err(reason) => {
                    crate::crumbs::record("main_sample", format_args!("unavailable={reason}"));
                    String::new()
                }
            };
            if let Err(error) = crate::crash::record(
                state.local_data_root(),
                crate::crash::Kind::Hang,
                &format!("main thread unresponsive for {ms}ms; main_scope={scope}{census}"),
                crate::crash::Origin {
                    thread: Some("main"),
                    backtrace: Some(&frames),
                    ..crate::crash::Origin::NONE
                },
            ) {
                crate::note_window_event(
                    state.local_data_root(),
                    &format!("crash evidence: {error}"),
                );
            }
        }
        if let Some(ms) = tick.badge {
            let _ = app.emit_to(
                crate::MAIN_WINDOW_LABEL,
                "crash:hang",
                serde_json::json!({"ms":ms, "count":self.watch.count}),
            );
        }
    }
}

/// Names only, never event payloads such as paths or URLs.
pub(crate) fn event_name(event: &tauri::RunEvent) -> &'static str {
    match event {
        tauri::RunEvent::Exit => "event_exit",
        tauri::RunEvent::ExitRequested { .. } => "event_exit_requested",
        tauri::RunEvent::WindowEvent { event, .. } => match event {
            tauri::WindowEvent::Resized(_) => "event_window_resized",
            tauri::WindowEvent::Focused(_) => "event_window_focused",
            tauri::WindowEvent::CloseRequested { .. } => "event_window_close",
            tauri::WindowEvent::Destroyed => "event_window_destroyed",
            _ => "event_window",
        },
        _ => "event_loop_callback",
    }
}

/// Wall-clock milliseconds, the unit crumbs and censuses are stamped in.
fn wall_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_millis() as u64)
}

/// Whether any screen is awake to show the window. CoreGraphics counts a
/// display active only while it is awake and drawable, and a lid-closed
/// DarkWake has none. Asked from this thread on 2026-09-24 under load
/// average 62–105: p50 7.5 µs, p99 23 µs per question.
#[cfg(target_os = "macos")]
fn screens_awake() -> bool {
    let mut count = 0u32;
    // SAFETY: a zero capacity with a null list asks for the count alone,
    // which CoreGraphics writes through the one pointer, valid for the call.
    let error = unsafe {
        objc2_core_graphics::CGGetActiveDisplayList(0, std::ptr::null_mut(), &raw mut count)
    };
    // A failed question says nothing about the screens: judging stays on.
    error != objc2_core_graphics::CGError::Success || count > 0
}

/// Elsewhere nothing is asked: the window is taken to be seen.
#[cfg(not(target_os = "macos"))]
fn screens_awake() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A beat of an observer that was there the whole time: its nap took
    /// the one beat it asked for, its two clocks agree, and a screen is on.
    fn beat(now: u64) -> Beat {
        Beat {
            now,
            wall: now,
            nap: Limits::DEFAULT.ping_ms,
            seen: true,
        }
    }

    #[test]
    fn delayed_main_loop_crosses_two_thresholds_once_and_warms_up() {
        let limits = Limits::DEFAULT;
        let mut watch = Watch::new(0);
        assert_eq!(
            watch.step(beat(limits.warmup_ms - 1), None, limits),
            Tick::default()
        );
        let begin = limits.warmup_ms;
        assert!(watch.step(beat(begin), None, limits).ping);
        assert_eq!(
            watch.step(beat(begin + limits.first_ms - 1), None, limits),
            Tick::default()
        );
        assert_eq!(
            watch.step(beat(begin + limits.first_ms), None, limits).hang,
            Some(limits.first_ms)
        );
        assert_eq!(watch.count, 1);
        assert_eq!(
            watch
                .step(beat(begin + limits.second_ms), None, limits)
                .badge,
            Some(limits.second_ms)
        );
        assert_eq!(
            watch.step(beat(begin + limits.second_ms * 2), None, limits),
            Tick::default()
        );
        assert!(
            watch
                .step(
                    beat(begin + limits.second_ms * 2 + 1),
                    Some(begin + limits.second_ms * 2 + 1),
                    limits
                )
                .ping
        );
        assert_eq!(watch.count, 1);
        eprintln!(
            "hang timeline: warmup 0..{begin}ms; ping {begin}ms; crumb {}ms; badge {}ms; recovered {}ms; count=1",
            begin + limits.first_ms,
            begin + limits.second_ms,
            begin + limits.second_ms * 2 + 1
        );
    }

    #[test]
    fn disabling_cancels_an_episode_and_reenable_warms_up() {
        let mut watch = Watch::new(0);
        let limits = Limits::DEFAULT;
        assert!(watch.step(beat(limits.warmup_ms), None, limits).ping);
        let off = Limits {
            watchdog: false,
            ..limits
        };
        assert_eq!(
            watch.step(beat(limits.warmup_ms + limits.second_ms), None, off),
            Tick::default()
        );
        assert_eq!(watch.count, 0);
        assert_eq!(
            watch.step(beat(limits.warmup_ms + limits.second_ms + 1), None, limits),
            Tick::default()
        );
    }

    #[test]
    fn a_delayed_monitor_does_not_accuse_a_healthy_main_thread() {
        let limits = Limits::DEFAULT;
        let mut watch = Watch::new(0);
        watch.step(beat(limits.warmup_ms), None, limits);
        let tick = watch.step(
            beat(limits.warmup_ms + limits.second_ms),
            Some(limits.warmup_ms + 1),
            limits,
        );
        assert_eq!(tick.hang, None);
        assert_eq!(tick.badge, None);
        assert_eq!(watch.count, 0);
    }

    #[test]
    fn a_late_answer_still_records_the_missed_thresholds() {
        let limits = Limits::DEFAULT;
        let mut watch = Watch::new(0);
        watch.step(beat(limits.warmup_ms), None, limits);
        let tick = watch.step(
            beat(limits.warmup_ms + limits.second_ms),
            Some(limits.warmup_ms + limits.second_ms),
            limits,
        );
        assert_eq!(tick.hang, Some(limits.second_ms));
        assert_eq!(tick.badge, Some(limits.second_ms));
        assert!(tick.ping);
    }

    /// 2026-09-24 05:53:39, lid closed: the ping left inside a DarkWake,
    /// macOS ran this process not at all for 34 s, and the main thread
    /// answered the moment it was run — before the observer, whose own
    /// 250 ms nap took as long. The report said 34,369 ms
    /// `answered_before_observation`; nobody had been waiting on anything.
    #[test]
    fn a_process_macos_did_not_run_is_not_a_hung_main_thread() {
        let limits = Limits::DEFAULT;
        let mut watch = Watch::new(0);
        let sent = limits.warmup_ms;
        assert!(watch.step(beat(sent), None, limits).ping);
        let answered = sent + 34_369;
        let back = answered + 5;
        let tick = watch.step(
            Beat {
                nap: back - sent,
                ..beat(back)
            },
            Some(answered),
            limits,
        );
        assert_eq!((tick.hang, tick.badge, tick.ping), (None, None, false));
        assert_eq!(
            tick.voided,
            Some(Voided {
                ms: 34_369,
                why: Absence::Late(back - sent - limits.ping_ms),
            })
        );
        assert_eq!(watch.count, 0);
        // The witness warms up again before it judges, as after a sleep.
        assert!(!watch.step(beat(back + limits.ping_ms), None, limits).ping);
        assert!(watch.step(beat(back + limits.warmup_ms), None, limits).ping);
    }

    /// 2026-09-23 07:38:41 → 07:40:13: the observer was writing a report
    /// when the lid-closed machine went to sleep for 90 s, so no nap of its
    /// own straddled the sleep and no "system slept" line was written — but
    /// between its two beats the wall clock ran 92 s and uptime 15 s. The
    /// report said 14,906 ms.
    #[test]
    fn a_sleep_spent_outside_the_nap_still_withdraws_the_episode() {
        let limits = Limits::DEFAULT;
        let mut watch = Watch::new(0);
        let sent = limits.warmup_ms;
        assert!(watch.step(beat(sent), None, limits).ping);
        let answered = sent + 14_906;
        let back = answered + 3;
        let wall = sent + 92_000;
        let tick = watch.step(Beat { wall, ..beat(back) }, Some(answered), limits);
        assert_eq!((tick.hang, tick.badge), (None, None));
        assert_eq!(
            tick.voided,
            Some(Voided {
                ms: 14_906,
                why: Absence::Slept((wall - sent) - (back - sent)),
            })
        );
        assert_eq!(watch.count, 0);
    }

    /// 2026-09-23 22:10–22:24: on-time beats inside a lid-closed DarkWake,
    /// and a main thread two seconds slow to answer them. Nobody could
    /// have seen that window, and none of it is judged until a screen is
    /// awake again and the watch has warmed up.
    #[test]
    fn a_window_no_screen_can_show_is_not_judged() {
        let limits = Limits::DEFAULT;
        let mut watch = Watch::new(0);
        let mut now = limits.warmup_ms;
        assert!(watch.step(beat(now), None, limits).ping);
        let dark = |now| Beat {
            seen: false,
            ..beat(now)
        };
        while now < limits.warmup_ms + limits.second_ms * 2 {
            now += limits.ping_ms;
            let tick = watch.step(dark(now), None, limits);
            assert_eq!((tick.ping, tick.hang, tick.badge), (false, None, None));
        }
        assert_eq!(watch.count, 0);
        now += limits.ping_ms;
        assert!(!watch.step(beat(now), None, limits).ping, "warms up first");
        assert!(watch.step(beat(now + limits.warmup_ms), None, limits).ping);
    }

    /// The same day's load, measured on this road: at load average 88–117
    /// on 12 cores the worst 250 ms nap overshot by 21.3 ms. An observer
    /// that late on every beat is still there, and still sees a main
    /// thread that never answers.
    #[test]
    fn the_worst_measured_load_jitter_still_testifies() {
        let limits = Limits::DEFAULT;
        let jitter = 22;
        let mut watch = Watch::new(0);
        let mut now = limits.warmup_ms;
        assert!(watch.step(beat(now), None, limits).ping);
        let hang = loop {
            now += limits.ping_ms + jitter;
            let tick = watch.step(
                Beat {
                    nap: limits.ping_ms + jitter,
                    ..beat(now)
                },
                None,
                limits,
            );
            assert_eq!(tick.voided, None);
            if let Some(ms) = tick.hang {
                break ms;
            }
        };
        assert!(hang >= limits.first_ms && hang < limits.first_ms + limits.ping_ms + jitter);
        assert_eq!(watch.count, 1);
    }

    /// A hang first seen still unanswered is reported at the first
    /// threshold, whatever its length; the beat its answer arrives on says
    /// how long it really was, once. One answered before it was seen needs
    /// no second line: the hang line already carries its whole length.
    #[test]
    fn a_hang_seen_open_says_how_long_it_really_lasted() {
        let limits = Limits::DEFAULT;
        let mut watch = Watch::new(0);
        let sent = limits.warmup_ms;
        let answered = sent + 9_000;
        let mut now = sent;
        assert!(watch.step(beat(now), None, limits).ping);
        let mut seen_at = None;
        while now + limits.ping_ms < answered {
            now += limits.ping_ms;
            let tick = watch.step(beat(now), None, limits);
            assert_eq!(tick.ended, None);
            seen_at = seen_at.or(tick.hang);
        }
        assert_eq!(seen_at, Some(limits.first_ms));
        let tick = watch.step(beat(now + limits.ping_ms), Some(answered), limits);
        assert_eq!(tick.ended, Some(9_000));
        assert!(tick.ping);
        assert_eq!(
            watch
                .step(beat(now + limits.ping_ms * 2), None, limits)
                .ended,
            None
        );

        let mut quick = Watch::new(0);
        assert!(quick.step(beat(sent), None, limits).ping);
        let tick = quick.step(beat(sent + 2_300), Some(sent + 2_266), limits);
        assert_eq!((tick.hang, tick.ended), (Some(2_266), None));
    }
}
