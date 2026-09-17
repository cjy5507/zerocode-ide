//! Main-thread ping state machine; rides the existing system-resume thread.

use std::sync::Arc;
use std::sync::atomic::{
    AtomicBool, AtomicU64,
    Ordering::{AcqRel, Acquire, Relaxed, Release},
};
use std::time::Instant;
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
}

struct Watch {
    warm_until: u64,
    pending: Option<u64>,
    noted: bool,
    badged: bool,
    count: u64,
}

impl Watch {
    fn new(now: u64) -> Self {
        Self {
            warm_until: now + Limits::DEFAULT.warmup_ms,
            pending: None,
            noted: false,
            badged: false,
            count: 0,
        }
    }
    fn step(&mut self, now: u64, answered: Option<u64>, limits: Limits) -> Tick {
        let mut tick = Tick::default();
        if !limits.watchdog {
            self.warm_until = now.saturating_add(limits.warmup_ms);
            self.pending = None;
            return tick;
        }
        if now < self.warm_until {
            return tick;
        }
        if let Some(sent) = self.pending {
            // Judge when the main thread answered, not when THIS thread was
            // next scheduled. A busy diagnostics thread cannot frame a healthy UI.
            let ms = answered.unwrap_or(now).saturating_sub(sent);
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
    pub(crate) fn tick(&mut self, app: &tauri::AppHandle) {
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
        let tick = self.watch.step(now, answered, limits);
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delayed_main_loop_crosses_two_thresholds_once_and_warms_up() {
        let limits = Limits::DEFAULT;
        let mut watch = Watch::new(0);
        assert_eq!(
            watch.step(limits.warmup_ms - 1, None, limits),
            Tick::default()
        );
        let begin = limits.warmup_ms;
        assert!(watch.step(begin, None, limits).ping);
        assert_eq!(
            watch.step(begin + limits.first_ms - 1, None, limits),
            Tick::default()
        );
        assert_eq!(
            watch.step(begin + limits.first_ms, None, limits).hang,
            Some(limits.first_ms)
        );
        assert_eq!(watch.count, 1);
        assert_eq!(
            watch.step(begin + limits.second_ms, None, limits).badge,
            Some(limits.second_ms)
        );
        assert_eq!(
            watch.step(begin + limits.second_ms * 2, None, limits),
            Tick::default()
        );
        assert!(
            watch
                .step(
                    begin + limits.second_ms * 2 + 1,
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
        assert!(watch.step(limits.warmup_ms, None, limits).ping);
        let off = Limits {
            watchdog: false,
            ..limits
        };
        assert_eq!(
            watch.step(limits.warmup_ms + limits.second_ms, None, off),
            Tick::default()
        );
        assert_eq!(watch.count, 0);
        assert_eq!(
            watch.step(limits.warmup_ms + limits.second_ms + 1, None, limits),
            Tick::default()
        );
    }

    #[test]
    fn a_delayed_monitor_does_not_accuse_a_healthy_main_thread() {
        let limits = Limits::DEFAULT;
        let mut watch = Watch::new(0);
        watch.step(limits.warmup_ms, None, limits);
        let tick = watch.step(
            limits.warmup_ms + limits.second_ms,
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
        watch.step(limits.warmup_ms, None, limits);
        let tick = watch.step(
            limits.warmup_ms + limits.second_ms,
            Some(limits.warmup_ms + limits.second_ms),
            limits,
        );
        assert_eq!(tick.hang, Some(limits.second_ms));
        assert_eq!(tick.badge, Some(limits.second_ms));
        assert!(tick.ping);
    }
}
