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
use zerocode_orchestrator::runtime_actor::Served;

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
    /// A hang seen still unanswered, said again every
    /// [`STILL_OPEN_EVERY_MS`] while it stays open.
    still: Option<u64>,
}

/// How often a hang still open is said again (t-75646): the 3 min 40 s of
/// 2026-10-26 07:30 left no line between its start and the person's close,
/// so nobody could tell a long hang from a short one ended by the close.
const STILL_OPEN_EVERY_MS: u64 = 10_000;

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
    /// The length a hang still open is said again at.
    still_at: u64,
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
            still_at: 0,
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
                self.still_at = ms.saturating_add(STILL_OPEN_EVERY_MS);
                tick.hang = Some(ms);
            }
            if !self.badged && ms >= limits.second_ms {
                self.badged = true;
                tick.badge = Some(ms);
            }
            if answered.is_none() {
                if seen_open && ms >= self.still_at {
                    self.still_at = ms.saturating_add(STILL_OPEN_EVERY_MS);
                    tick.still = Some(ms);
                }
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

/// The ledger actor's long requests, as the watch has said them (t-75646).
///
/// Both stands of 2026-10-26 ended with a goodbye that the actor did not
/// answer while the main thread took the close at once: the stand was the
/// actor's, and this watch only ever looked at the main thread. It reads what
/// the actor serves without asking it anything, and says a request that has
/// run past the first threshold — again while it runs, at most once per
/// [`STILL_OPEN_EVERY_MS`] — and its end once.
///
/// It judges on the monotonic clock the actor stamps a request with, and on
/// nothing else (t-75646 r3). That clock stops while the machine sleeps —
/// `CLOCK_UPTIME_RAW` on macOS, `CLOCK_MONOTONIC` on Linux — so a sleep
/// drops out of a request's length by itself, and the main-thread watch's
/// sleep judgment, which reads the wall clock and so takes a clock set ahead
/// for a sleep, neither pauses this watch nor times a request again. A beat
/// that comes back late is not taken back either: the actor held the request
/// all that time. On Windows `Instant` is the performance counter, and
/// whether it counts a sleep is not settled here: there a request a sleep
/// fell inside may be said with the sleep in its length.
#[derive(Debug, Default)]
struct ActorWatch {
    /// The request said busy, and the length its next line is due at.
    said: Option<(Served, u64)>,
    /// When the last busy line was said.
    last_line: Option<Instant>,
}

#[derive(Debug, PartialEq, Eq)]
enum ActorLine {
    Busy { verb: &'static str, ms: u64 },
    Answered { verb: &'static str, ms: u64 },
}

impl fmt::Display for ActorLine {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Busy { verb, ms } => write!(f, "ledger actor busy: {ms}ms on {verb}"),
            Self::Answered { verb, ms } => {
                write!(f, "ledger actor answered: {verb} after about {ms}ms")
            }
        }
    }
}

impl ActorWatch {
    /// `look` is the actor's [`Serving::peek`]: `None` when it could not be
    /// read this beat, which changes nothing.
    fn step(
        &mut self,
        look: Option<Option<Served>>,
        now: Instant,
        first_ms: u64,
    ) -> Vec<ActorLine> {
        let Some(serving) = look else {
            return Vec::new();
        };
        let ran = |served: Served| millis(now.saturating_duration_since(served.since));
        let mut lines = Vec::new();
        if let Some((said, _)) = self.said
            && serving != Some(said)
        {
            lines.push(ActorLine::Answered {
                verb: said.verb,
                ms: ran(said),
            });
            self.said = None;
        }
        if let Some(served) = serving {
            let ms = ran(served);
            let due = self.said.map_or(first_ms, |(_, due)| due);
            let quiet = self
                .last_line
                .is_none_or(|at| millis(now.saturating_duration_since(at)) >= STILL_OPEN_EVERY_MS);
            if ms >= due && quiet {
                lines.push(ActorLine::Busy {
                    verb: served.verb,
                    ms,
                });
                self.said = Some((served, ms.saturating_add(STILL_OPEN_EVERY_MS)));
                self.last_line = Some(now);
            }
        }
        lines
    }
}

fn millis(elapsed: Duration) -> u64 {
    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
}

/// What the sample taken after a hang was seen can speak for (t-72138).
///
/// `sample` starts only once the hang is seen and then runs a second, so a
/// main thread that answers just past the threshold leaves the sample its
/// idle loop: the 2,031 ms of 2026-10-23 05:19 inside Chromium's message
/// pump was filed with `mach_msg` in 72 of 78 samples as its stack.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Sampled {
    /// Still unanswered when the sample ended: every frame is the hang's.
    Open,
    /// Answered this many ms after the ping, before the sample began: no
    /// frame is the hang's.
    Before(u64),
    /// Answered this many ms after the ping, while the sample ran: the
    /// frames hold the hang's end and then the idle loop.
    During(u64),
}

/// `sent` and `answered` are the ping's and its answer's ms on the monitor's
/// clock, and `sample_from` is when the sampler was started on the same one.
fn sampled(sent: u64, sample_from: u64, answered: Option<u64>) -> Sampled {
    match answered {
        None => Sampled::Open,
        Some(at) if at < sample_from => Sampled::Before(at.saturating_sub(sent)),
        Some(at) => Sampled::During(at.saturating_sub(sent)),
    }
}

impl Sampled {
    /// The frames the hang's record may carry.
    fn frames(
        self,
        sample: Result<Vec<String>, &'static str>,
    ) -> Result<Vec<String>, &'static str> {
        match self {
            Self::Before(_) => sample.and(Err("answered_before_sample")),
            Self::Open | Self::During(_) => sample,
        }
    }
}

impl fmt::Display for Sampled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Open => Ok(()),
            Self::Before(ms) => write!(f, " lasted_ms={ms} answered_before_sample"),
            Self::During(ms) => write!(f, " lasted_ms={ms} answered_during_sample"),
        }
    }
}

/// The one main-thread closure the observer keeps queued, and its answer.
struct Answer {
    outstanding: AtomicBool,
    at_ms: AtomicU64,
}

impl Answer {
    fn new() -> Self {
        Self {
            outstanding: AtomicBool::new(false),
            at_ms: AtomicU64::new(0),
        }
    }

    /// The main thread's answer to the last ping, taken; `None` while the
    /// closure still waits in the queue.
    fn take(&self) -> Option<u64> {
        (!self.outstanding.load(Acquire))
            .then(|| self.at_ms.swap(0, Relaxed))
            .filter(|at| *at != 0)
    }

    /// [`Self::take`] without taking: the next beat says the end.
    fn peek(&self) -> Option<u64> {
        (!self.outstanding.load(Acquire))
            .then(|| self.at_ms.load(Relaxed))
            .filter(|at| *at != 0)
    }

    /// Claim the queue's one slot for a new ping; `false` while the last
    /// closure is still queued.
    fn claim(&self) -> bool {
        self.outstanding
            .compare_exchange(false, true, AcqRel, Acquire)
            .is_ok()
    }

    /// What the queued closure does on the main thread.
    fn answer(&self, at_ms: u64) {
        self.at_ms.store(at_ms, Relaxed);
        self.outstanding.store(false, Release);
    }

    /// A closure that could not be queued holds no slot.
    fn release(&self) {
        self.outstanding.store(false, Release);
    }
}

/// One beat against the answer slot.
struct Beaten {
    tick: Tick,
    answered: Option<u64>,
    /// Whether a new closure must go on the main thread's queue.
    queue: bool,
}

/// The watch's judgment for one beat, and whether it queues a ping.
///
/// A ping the watch asks for while the last closure is still queued is not
/// queued again — a closure cannot be taken back out of the queue — and
/// the watch goes on judging from this beat: the main thread has not run
/// its queue since at least now.
fn beat_once(watch: &mut Watch, answer: &Answer, beat: Beat, limits: Limits) -> Beaten {
    let answered = answer.take();
    let tick = watch.step(beat, answered, limits);
    let queue = tick.ping && answer.claim();
    Beaten {
        tick,
        answered,
        queue,
    }
}

pub(crate) struct Monitor {
    start: Instant,
    watch: Watch,
    answer: Arc<Answer>,
    armed: bool,
    actor: ActorWatch,
}

impl Monitor {
    pub(crate) fn new() -> Self {
        Self {
            start: Instant::now(),
            watch: Watch::new(0),
            answer: Arc::new(Answer::new()),
            armed: false,
            actor: ActorWatch::default(),
        }
    }
    pub(crate) fn resumed(&mut self) {
        self.armed = false;
    }

    /// One beat's judgments, the main thread's and the actor's, without the
    /// app: `look` reads what the actor serves.
    fn judge(
        &mut self,
        beat: Beat,
        limits: Limits,
        look: impl FnOnce() -> Option<Option<Served>>,
    ) -> (Beaten, Vec<ActorLine>) {
        if !self.armed {
            let count = self.watch.count;
            self.watch = Watch::new(beat.now);
            self.watch.count = count;
            self.armed = true;
        }
        let beaten = beat_once(&mut self.watch, &self.answer, beat, limits);
        // The actor is judged on its own clock, whatever the watch's warm-up
        // or sleep judgment says; only a watch turned off judges nothing. It
        // forgets the request it said, but not when it last said one, so a
        // watch turned off and on keeps ten seconds between busy lines.
        if !limits.watchdog {
            self.actor.said = None;
            return (beaten, Vec::new());
        }
        let now = self.start + Duration::from_millis(beat.now);
        let actor = self.actor.step(look(), now, limits.first_ms);
        (beaten, actor)
    }
    /// One beat, after a nap that really took `nap` of wall time.
    pub(crate) fn tick(&mut self, app: &tauri::AppHandle, nap: Duration) {
        let now = self.start.elapsed().as_millis() as u64;
        if !READY.load(Relaxed) {
            return;
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
        let beat = Beat {
            now,
            wall: wall_ms(),
            nap: u64::try_from(nap.as_millis()).unwrap_or(u64::MAX),
            seen: screens_awake(),
        };
        let (
            Beaten {
                tick,
                answered,
                queue,
            },
            actor,
        ) = self.judge(beat, limits, crate::orchestration::actor_serving);
        for line in actor {
            crate::crumbs::record("ledger_actor", format_args!("{line}"));
            crate::note_window_event(
                app.state::<crate::AppState>().local_data_root(),
                &line.to_string(),
            );
        }
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
        if queue {
            // A disabled/resumed monitor can abandon a judgment, but it cannot
            // cancel a queued Tauri closure. Keep that queue slot until it answers.
            let answer = Arc::clone(&self.answer);
            let start = self.start;
            if app
                .run_on_main_thread(move || answer.answer(start.elapsed().as_millis() as u64))
                .is_err()
            {
                self.answer.release();
                self.armed = false;
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
        if let Some(ms) = tick.still {
            // Said from the scope the main thread still stands in: nothing has
            // run on it since the hang line, so the scope is the hang's own.
            let scope = crate::crumbs::main_scope();
            let io = main_io_now(wall_ms());
            crate::crumbs::record(
                "hang_still_open",
                format_args!("ms={ms} main_scope={scope}"),
            );
            crate::note_window_event(
                app.state::<crate::AppState>().local_data_root(),
                &format!("hang still open: {ms}ms main_scope={scope} io={io}"),
            );
        }
        if let Some(ms) = tick.hang {
            let (scope, io) = if answered.is_none() {
                (crate::crumbs::main_scope(), main_io_now(wall_ms()))
            } else {
                let word = "answered_before_observation".to_string();
                (word.clone(), word)
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
            crate::crumbs::record("hang_io", format_args!("{io}"));
            let state = app.state::<crate::AppState>();
            crate::note_window_event(
                state.local_data_root(),
                &format!("hang: {ms}ms main_scope={scope} last_command={command}{census}"),
            );
            // The disk work the main thread stands in, if any (t-67102): the
            // 20 255 ms of 2026-10-21 had to be traced to a settings read by
            // hand, from the command's name alone.
            crate::note_window_event(state.local_data_root(), &format!("hang io: {io}"));
            let (sample, sampled) = if answered.is_none() {
                let sample_from = self.start.elapsed().as_millis() as u64;
                let sample = crate::hang_sample::sample_main_thread();
                // Peeked, not taken: the next beat takes it and says the end.
                let answered = self.answer.peek();
                // Unanswered at this beat, so the hang is the time since the ping.
                let sampled = sampled(now.saturating_sub(ms), sample_from, answered);
                (sampled.frames(sample), sampled)
            } else {
                (Err("answered_before_observation"), Sampled::Open)
            };
            let frames = match sample {
                Ok(frames) => {
                    for frame in &frames {
                        crate::crumbs::record(
                            crate::hang_sample::SAMPLE_CRUMB,
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
                &format!(
                    "main thread unresponsive for {ms}ms; main_scope={scope}{census}{sampled}"
                ),
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

/// The main thread's disk work for a hang line: the function and the file it
/// stands in and for how long, or `none`.
fn main_io_now(now_ms: u64) -> String {
    crate::crumbs::main_io().map_or_else(
        || "none".to_string(),
        |io| format!("{} for {}ms", io.line, now_ms.saturating_sub(io.at_ms)),
    )
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
        // Still open: no second hang, no second badge, no new ping — at most
        // the line that says it is still open.
        assert_eq!(
            watch.step(beat(begin + limits.second_ms * 2), None, limits),
            Tick {
                still: Some(limits.second_ms * 2),
                ..Tick::default()
            }
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

    /// 2026-10-26 07:30 and 15:21 (t-75646): the window stood for minutes and
    /// the watch wrote nothing. One beat back late inside a hang's first two
    /// seconds withdraws the judgment and warms the watch up; when the warm-up
    /// ends the ping is still queued, so no second closure can be, and the
    /// watch used to drop its judgment on every beat after — silent until the
    /// main thread answered, however long that took. The watch judges from
    /// its return instead: the main thread has not run since at least then.
    #[test]
    fn a_hang_whose_start_the_observer_missed_is_judged_from_its_return() {
        let limits = Limits::DEFAULT;
        let mut watch = Watch::new(0);
        let answer = Answer::new();
        let sent = limits.warmup_ms;
        assert!(beat_once(&mut watch, &answer, beat(sent), limits).queue);
        let late = sent + limits.first_ms / 2;
        let missed = beat_once(
            &mut watch,
            &answer,
            Beat {
                nap: limits.ping_ms * 3,
                ..beat(late)
            },
            limits,
        );
        assert_eq!(missed.tick, Tick::default());
        let back = late + limits.warmup_ms;
        let mut now = late;
        let mut seen = None;
        while now < back + limits.second_ms {
            now += limits.ping_ms;
            let beaten = beat_once(&mut watch, &answer, beat(now), limits);
            assert!(!beaten.queue, "the queued closure keeps the one slot");
            seen = seen.or(beaten.tick.hang.map(|ms| (now, ms)));
        }
        assert_eq!(seen, Some((back + limits.first_ms, limits.first_ms)));
        answer.answer(now);
        let ended = beat_once(&mut watch, &answer, beat(now + limits.ping_ms), limits);
        assert_eq!(ended.tick.ended, Some(now - back));
        assert!(ended.queue, "the slot is free again once answered");
    }

    /// A hang still open is said again every ten seconds, and only while it
    /// is open: the line written at the first threshold is not the last word
    /// about a hang the person ends by closing the window.
    #[test]
    fn a_hang_still_open_is_said_again_every_ten_seconds() {
        let limits = Limits::DEFAULT;
        let mut watch = Watch::new(0);
        let sent = limits.warmup_ms;
        assert!(watch.step(beat(sent), None, limits).ping);
        let mut now = sent;
        let mut still = Vec::new();
        while now < sent + 35_000 {
            now += limits.ping_ms;
            still.extend(watch.step(beat(now), None, limits).still);
        }
        assert_eq!(
            still,
            [
                limits.first_ms + STILL_OPEN_EVERY_MS,
                limits.first_ms + STILL_OPEN_EVERY_MS * 2,
                limits.first_ms + STILL_OPEN_EVERY_MS * 3,
            ]
        );
        let tick = watch.step(beat(now + limits.ping_ms), Some(now), limits);
        assert_eq!((tick.still, tick.ended), (None, Some(now - sent)));
        assert_eq!(watch.count, 1, "one hang, however often it is said");
    }

    /// Made-up moments for the actor watch alone: `ms` past one origin.
    struct Clock(Instant);

    impl Clock {
        fn at(&self, ms: u64) -> Instant {
            self.0 + Duration::from_millis(ms)
        }

        fn served(&self, verb: &'static str, since_ms: u64) -> Option<Option<Served>> {
            Some(Some(Served {
                verb,
                since: self.at(since_ms),
            }))
        }
    }

    /// A request the actor serves for 25 s is said at the first threshold and
    /// every ten seconds after, and its end once; short ones say nothing.
    #[test]
    fn a_long_actor_request_is_said_while_it_runs_and_once_when_it_ends() {
        let first = Limits::DEFAULT.first_ms;
        let clock = Clock(Instant::now());
        let mut watch = ActorWatch::default();
        let mut said = Vec::new();
        let mut step = |look, now| watch.step(look, clock.at(now), first);
        assert!(step(clock.served("check", 0), 1_000).is_empty());
        assert!(step(Some(None), 1_500).is_empty());
        for now in (0..=25_000).step_by(250) {
            said.extend(step(clock.served("window_exiting", 2_000), 2_000 + now));
        }
        said.extend(step(Some(None), 27_250));
        assert!(step(Some(None), 27_500).is_empty());
        assert_eq!(
            said,
            [
                ActorLine::Busy {
                    verb: "window_exiting",
                    ms: 2_000
                },
                ActorLine::Busy {
                    verb: "window_exiting",
                    ms: 12_000
                },
                ActorLine::Busy {
                    verb: "window_exiting",
                    ms: 22_000
                },
                ActorLine::Answered {
                    verb: "window_exiting",
                    ms: 25_250
                },
            ]
        );
        assert_eq!(
            said[0].to_string(),
            "ledger actor busy: 2000ms on window_exiting"
        );
    }

    /// A beat that meets the actor writing reads nothing and says nothing;
    /// a run of requests each just past the threshold says one busy line per
    /// ten seconds, not one per request.
    #[test]
    fn actor_lines_are_at_most_one_busy_line_per_ten_seconds() {
        let first = Limits::DEFAULT.first_ms;
        let clock = Clock(Instant::now());
        let mut watch = ActorWatch::default();
        let mut busy = 0;
        let mut since = 0;
        for now in (0..60_000).step_by(250) {
            if now - since >= 2_500 {
                since = now;
            }
            if now % 1_000 == 0 {
                assert!(watch.step(None, clock.at(now), first).is_empty());
            }
            busy += watch
                .step(clock.served("view", since), clock.at(now), first)
                .iter()
                .filter(|line| matches!(line, ActorLine::Busy { .. }))
                .count();
        }
        assert!((1..=6).contains(&busy), "{busy} busy lines in 60 s");
    }

    /// A monitor on made-up clocks (t-75646 r2): uptime, which every
    /// judgment runs on and which stops while the machine sleeps, and the
    /// wall clock, which a sleep runs ahead and a person or ntp can set back
    /// or forward. The main thread answers every ping at once, so only the
    /// actor is judged.
    struct Machine {
        monitor: Monitor,
        up: u64,
        wall: u64,
        limits: Limits,
        lines: Vec<ActorLine>,
    }

    impl Machine {
        /// A monitor already warmed up and watching.
        fn watching() -> Self {
            let mut machine = Self {
                monitor: Monitor::new(),
                up: 0,
                wall: 1_800_000_000_000,
                limits: Limits::DEFAULT,
                lines: Vec::new(),
            };
            machine.run(Limits::DEFAULT.warmup_ms + Limits::DEFAULT.ping_ms, None);
            machine
        }

        /// What the actor stamps a request it takes now with.
        fn take(&self, verb: &'static str) -> Served {
            Served {
                verb,
                since: self.monitor.start + Duration::from_millis(self.up),
            }
        }

        /// One beat after a nap that took `nap` ms of uptime and of wall time.
        fn beat(&mut self, nap: u64, serving: Option<Served>) {
            self.up += nap;
            self.wall += nap;
            let beat = Beat {
                now: self.up,
                wall: self.wall,
                nap,
                seen: true,
            };
            let (beaten, lines) = self.monitor.judge(beat, self.limits, || Some(serving));
            if beaten.queue {
                self.monitor.answer.answer(self.up);
            }
            self.lines.extend(lines);
        }

        /// On-time beats for `ms`, the actor serving `serving` throughout.
        fn run(&mut self, ms: u64, serving: Option<Served>) {
            for _ in 0..ms / Limits::DEFAULT.ping_ms {
                self.beat(Limits::DEFAULT.ping_ms, serving);
            }
        }

        /// A sleep the nap straddled: the resume thread says so instead of
        /// beating, as `main` does. `main` tells it only by the wall clock
        /// running `ms` past the nap, so a clock set ahead inside a nap is
        /// this too.
        fn sleep(&mut self, ms: u64) {
            self.up += Limits::DEFAULT.ping_ms;
            self.wall += Limits::DEFAULT.ping_ms + ms;
            self.monitor.resumed();
        }

        fn busy(&self) -> Vec<u64> {
            self.lines
                .iter()
                .filter_map(|line| match line {
                    ActorLine::Busy { ms, .. } => Some(*ms),
                    ActorLine::Answered { .. } => None,
                })
                .collect()
        }
    }

    /// The wall clock set back an hour while the actor stands: the request is
    /// timed on uptime, so it is said at the first threshold and every ten
    /// seconds after as if nothing happened — the wall clock used to make it
    /// zero, and the stand went unsaid until the clock caught up.
    #[test]
    fn an_actor_request_is_said_on_time_when_the_wall_clock_goes_back() {
        let mut machine = Machine::watching();
        let request = machine.take("window_exiting");
        machine.run(1_000, Some(request));
        machine.wall -= 3_600_000;
        machine.run(24_000, Some(request));
        machine.run(250, None);
        assert_eq!(
            machine.lines,
            [
                ActorLine::Busy {
                    verb: "window_exiting",
                    ms: 2_000
                },
                ActorLine::Busy {
                    verb: "window_exiting",
                    ms: 12_000
                },
                ActorLine::Busy {
                    verb: "window_exiting",
                    ms: 22_000
                },
                ActorLine::Answered {
                    verb: "window_exiting",
                    ms: 25_250
                },
            ]
        );
    }

    /// The lines a request the actor stands on for `ms` is said with, from
    /// the moment it was taken, ending a beat after it is answered.
    fn stood(ms: u64) -> Vec<ActorLine> {
        let mut lines: Vec<ActorLine> = (Limits::DEFAULT.first_ms..ms)
            .step_by(STILL_OPEN_EVERY_MS as usize)
            .map(|ms| ActorLine::Busy {
                verb: "window_exiting",
                ms,
            })
            .collect();
        lines.push(ActorLine::Answered {
            verb: "window_exiting",
            ms: ms + Limits::DEFAULT.ping_ms,
        });
        lines
    }

    /// The wall clock set an hour either way a second into a request, with
    /// no sleep (t-75646 r3): the request is timed on the monotonic clock
    /// alone, so it is said at the first threshold, every ten seconds after
    /// and at its end. A clock set ahead used to read as a sleep and blind
    /// the actor watch for the watch's 15 s warm-up: a 10 s stand left no
    /// line at all, and a 25 s one was first said at about 18 s.
    #[test]
    fn an_actor_request_is_said_on_time_whichever_way_the_wall_clock_is_set() {
        for set in [3_600_000, -3_600_000_i64] {
            for ms in [10_000, 25_000] {
                let mut machine = Machine::watching();
                let request = machine.take("window_exiting");
                machine.run(1_000, Some(request));
                machine.wall = machine.wall.saturating_add_signed(set);
                machine.run(ms - 1_000, Some(request));
                machine.run(250, None);
                assert_eq!(machine.lines, stood(ms), "set {set} ms, {ms} ms stand");
            }
        }
    }

    /// The wall clock set an hour ahead inside a nap reads to `main` as a
    /// sleep, and the monitor is told it resumed: the actor watch still
    /// times the request on the monotonic clock and says it on time.
    #[test]
    fn an_actor_request_is_said_on_time_when_the_wall_clock_jumps_inside_a_nap() {
        for ms in [10_000, 25_000] {
            let mut machine = Machine::watching();
            let request = machine.take("window_exiting");
            machine.run(1_000, Some(request));
            machine.sleep(3_600_000);
            machine.run(ms - 1_000 - Limits::DEFAULT.ping_ms, Some(request));
            machine.run(250, None);
            assert_eq!(machine.lines, stood(ms), "{ms} ms stand");
        }
    }

    /// A watch turned off in the settings says nothing of the actor either.
    #[test]
    fn a_watch_turned_off_says_nothing_of_the_actor() {
        let mut machine = Machine::watching();
        machine.limits.watchdog = false;
        let request = machine.take("window_exiting");
        machine.run(25_000, Some(request));
        machine.run(250, None);
        assert!(machine.lines.is_empty(), "{:?}", machine.lines);
    }

    /// A watch turned off and on again a beat later keeps its ten seconds
    /// between busy lines (t-75646 r4): the request said at 2 s is not said
    /// again 500 ms later, and nothing is said while the watch is off.
    #[test]
    fn a_watch_turned_off_and_on_keeps_ten_seconds_between_busy_lines() {
        let mut machine = Machine::watching();
        let request = machine.take("window_exiting");
        machine.run(2_000, Some(request));
        machine.limits.watchdog = false;
        machine.beat(250, Some(request));
        machine.limits.watchdog = true;
        machine.run(9_750, Some(request));
        machine.run(250, None);
        assert_eq!(
            machine.lines,
            [
                ActorLine::Busy {
                    verb: "window_exiting",
                    ms: 2_000
                },
                ActorLine::Busy {
                    verb: "window_exiting",
                    ms: 12_000
                },
                ActorLine::Answered {
                    verb: "window_exiting",
                    ms: 12_250
                },
            ]
        );
    }

    /// A one-second request the machine slept 30 s in the middle of is not
    /// a busy actor: the monotonic clock stops while the machine sleeps, so
    /// the request ran one second of it, wherever the sleep fell. A request
    /// held 25 s with no sleep is still said at the first threshold.
    #[test]
    fn a_short_actor_request_across_a_sleep_says_nothing() {
        /// One way the machine sleeps while the request stands.
        type Away = fn(&mut Machine);
        let away: [(&str, Away); 2] = [
            ("slept in the nap", |machine| machine.sleep(30_000)),
            ("slept between beats", |machine| machine.wall += 30_000),
        ];
        for (how, away) in away {
            let mut machine = Machine::watching();
            let request = machine.take("view");
            machine.run(500, Some(request));
            away(&mut machine);
            machine.run(500, Some(request));
            machine.run(250, None);
            assert!(machine.lines.is_empty(), "{how}: {:?}", machine.lines);
        }
        let mut machine = Machine::watching();
        let request = machine.take("window_exiting");
        machine.run(25_000, Some(request));
        machine.run(250, None);
        assert_eq!(machine.lines, stood(25_000));
    }

    /// A beat that comes back 30 s late is not taken back for the actor
    /// (t-75646 r3): the process may not have been run, but the actor held
    /// the request all that time on the monotonic clock, so the request is
    /// said for all of it — and then every ten seconds, as ever.
    #[test]
    fn an_actor_request_held_through_a_late_beat_is_said_for_all_of_it() {
        let mut machine = Machine::watching();
        let request = machine.take("land_check");
        machine.run(1_500, Some(request));
        machine.beat(30_000, Some(request));
        machine.run(18_000, Some(request));
        machine.run(250, None);
        assert_eq!(machine.busy(), [31_500, 41_500]);
        assert_eq!(
            machine.lines.last(),
            Some(&ActorLine::Answered {
                verb: "land_check",
                ms: 49_750
            })
        );
    }

    /// 2026-10-23 05:19 (t-72138): the ping went unanswered 2,031 ms while
    /// the main thread ran Chromium's message pump, and the one-second
    /// sample that followed found it idle in `mach_msg` in 72 of its 78
    /// samples — the hang had ended, and its task carried the idle loop as
    /// its stack. A sample that began after the answer holds none of the
    /// hang; one the answer arrived in holds its end. Either way the record
    /// says when the answer came, so the frames are read for what they are.
    #[test]
    fn a_sample_taken_after_the_answer_is_not_filed_as_the_hangs_stack() {
        let limits = Limits::DEFAULT;
        let sent = limits.warmup_ms;
        let seen = sent + 2_031;
        let sample_from = seen + 4;
        let idle = || Ok(vec!["native::self_72::mach_msg2_trap".to_string()]);

        let before = sampled(sent, sample_from, Some(seen + 1));
        assert_eq!(before, Sampled::Before(2_032));
        assert_eq!(before.frames(idle()), Err("answered_before_sample"));
        assert_eq!(before.to_string(), " lasted_ms=2032 answered_before_sample");

        let during = sampled(sent, sample_from, Some(sample_from + 300));
        assert_eq!(during, Sampled::During(2_335));
        assert_eq!(during.frames(idle()), idle());
        assert_eq!(during.to_string(), " lasted_ms=2335 answered_during_sample");

        let open = sampled(sent, sample_from, None);
        assert_eq!(open, Sampled::Open);
        assert_eq!(open.frames(idle()), idle());
        assert_eq!(open.to_string(), "");
        // A sampler that failed says its own reason, whatever the answer.
        assert_eq!(during.frames(Err("sample_timeout")), Err("sample_timeout"));
    }
}
