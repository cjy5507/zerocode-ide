//! Keep the machine awake while agents work — Orca's `AgentAwakeService`
//! (agent-awake-service.ts:11-254), ported whole: a person walks away, the
//! laptop sleeps, and the agent they left working stops mid-task. The map
//! filed that as P0-17.
//!
//! The contract, measured:
//!
//!   - Three modes (`computer-awake-mode.ts`): `on` always blocks, `off`
//!     never, `auto` blocks while at least one agent is WORKING and was
//!     heard from within the last two hours — a wedged "working" pane must
//!     not hold the lid open forever (`AGENT_AWAKE_STATUS_STALE_AFTER_MS`).
//!   - macOS: `/usr/bin/caffeinate` as a child, killed to release. Orca
//!     runs `-i -s` beside Electron's display blocker; `-d` here is that
//!     blocker's own flag, so one child carries both halves. `-w <pid>`
//!     names the window, so the child ends when the window does by ANY
//!     road — `Drop` is the fast release of an orderly exit, never the only
//!     one. Measured 2026-09-20, before that flag: 83 `caffeinate -d -i -s`
//!     hanging off launchd, the oldest eight days old, one left behind per
//!     restart or crash, and a machine that had not slept for days.
//!   - Linux: `systemd-inhibit --what=sleep:handle-lid-switch` — logind
//!     ignores ordinary sleep inhibitors for the lid on many systems, so
//!     the lid lock rides along (linux-lid-sleep-assertion.ts:68).
//!   - Windows: `SetThreadExecutionState`, and deliberately NO lid
//!     modelling — Orca leaves it out because keeping a closed lid awake
//!     there means mutating the user's global power plan.
//!   - A child that dies unexpectedly is retried after thirty seconds
//!     (`MACOS_SYSTEM_SLEEP_ASSERTION_RETRY_MS`), not in a hot loop.
//!
//! One keeper thread owns every side effect. On Windows that is forced —
//! `ES_CONTINUOUS` binds to the calling thread, so a state set from a
//! transient command thread would lapse with it — and on the other
//! platforms it keeps the child's spawn/kill serialised without holding the
//! state lock across a process launch.

use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::TermId;

/// Orca's staleness horizon: a working status older than this no longer
/// counts toward keeping the machine up.
pub const STALE_AFTER: Duration = Duration::from_secs(2 * 60 * 60);

/// The pause before a dead assertion child is tried again.
const RETRY_AFTER: Duration = Duration::from_secs(30);

/// How often the keeper re-checks with nothing to wake it — the staleness
/// horizon has to be noticed even when no hook event arrives to prompt it.
const KEEPER_TICK: Duration = Duration::from_secs(60);

/// `on` | `off` | `auto` (`COMPUTER_AWAKE_MODES`). Off is the default the
/// way Orca's normalizer answers when nothing was ever chosen.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ComputerAwakeMode {
    On,
    #[default]
    Off,
    Auto,
}

/// Should the machine be held awake right now? The pure half of the
/// service — Orca's `getEligibleRunningStatusCount` folded into its
/// `shouldBlock` line — so the rule is provable without a process table.
/// `heard_at` carries each working pane's LAST report moment.
#[must_use]
pub fn should_block(mode: ComputerAwakeMode, heard_at: &[Instant], now: Instant) -> bool {
    match mode {
        ComputerAwakeMode::On => true,
        ComputerAwakeMode::Off => false,
        ComputerAwakeMode::Auto => heard_at
            .iter()
            .any(|at| now.saturating_duration_since(*at) <= STALE_AFTER),
    }
}

struct Inner {
    mode: ComputerAwakeMode,
    /// When each WORKING pane was last heard from. A pane in any other
    /// state has no row here — leaving is what releases the lid.
    working_heard_at: HashMap<TermId, Instant>,
    /// What the keeper is currently asserting, so status() answers what IS,
    /// not what was asked for.
    active: bool,
    /// Monotonic predicate for work submitted to the keeper. It closes the
    /// window where a nudge arrives after enforcement but before the keeper
    /// starts waiting, and lets observers wait for their preceding writes.
    requested_revision: u64,
    /// The newest requested revision folded into `active` by the keeper.
    applied_revision: u64,
    stop: bool,
}

impl Inner {
    fn request_update(&mut self) {
        self.requested_revision = self
            .requested_revision
            .checked_add(1)
            .expect("awake state revision exhausted");
    }
}

/// The window's one hold on the power system.
pub struct AwakeService {
    shared: Arc<(Mutex<Inner>, Condvar)>,
}

impl Default for AwakeService {
    fn default() -> Self {
        let shared = Arc::new((
            Mutex::new(Inner {
                mode: ComputerAwakeMode::default(),
                working_heard_at: HashMap::new(),
                active: false,
                requested_revision: 0,
                applied_revision: 0,
                stop: false,
            }),
            Condvar::new(),
        ));
        let keeper = Arc::clone(&shared);
        std::thread::spawn(move || keeper_loop(&keeper));
        Self { shared }
    }
}

impl AwakeService {
    /// The person's choice, applied now — a change takes effect on the
    /// keeper's next breath, which the nudge makes immediate.
    pub fn set_mode(&self, mode: ComputerAwakeMode) {
        let (lock, nudge) = &*self.shared;
        let mut inner = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.mode == mode {
            return;
        }
        inner.mode = mode;
        inner.request_update();
        drop(inner);
        nudge.notify_all();
    }

    /// One pane's state, through the same single door every state change
    /// already walks. `working` marks the row; anything else clears it.
    pub fn note_pane(&self, term: TermId, working: bool) {
        let (lock, nudge) = &*self.shared;
        let mut inner = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let changed = if working {
            inner.working_heard_at.insert(term, Instant::now());
            true
        } else {
            inner.working_heard_at.remove(&term).is_some()
        };
        if changed {
            inner.request_update();
        }
        drop(inner);
        if changed {
            nudge.notify_all();
        }
    }

    /// A pane that is gone holds nothing open.
    pub fn forget_pane(&self, term: TermId) {
        self.note_pane(term, false);
    }

    /// `(mode, active)` — Orca's `ComputerAwakeStatus`, read by the
    /// status bar's caffeinate segment through `computer_awake_status`.
    pub fn status(&self) -> (ComputerAwakeMode, bool) {
        let (lock, changed) = &*self.shared;
        let mut inner = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let target_revision = inner.requested_revision;
        while inner.applied_revision < target_revision {
            inner = changed
                .wait(inner)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
        (inner.mode, inner.active)
    }
}

impl Drop for AwakeService {
    fn drop(&mut self) {
        let (lock, nudge) = &*self.shared;
        if let Ok(mut inner) = lock.lock() {
            inner.stop = true;
        }
        nudge.notify_all();
    }
}

/// The keeper: recompute, enforce, sleep until nudged or a minute passes.
/// The assertion child (or the Windows thread state) is owned HERE and
/// nowhere else.
fn keeper_loop(shared: &Arc<(Mutex<Inner>, Condvar)>) {
    let (lock, nudge) = &**shared;
    let mut child: Option<std::process::Child> = None;
    let mut retry_not_before: Option<Instant> = None;
    loop {
        let mut inner = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if inner.stop {
            drop(inner);
            release_assertion(&mut child);
            #[cfg(windows)]
            windows_execution_state(false);
            return;
        }
        let now = Instant::now();
        let heard: Vec<Instant> = inner.working_heard_at.values().copied().collect();
        let wanted = should_block(inner.mode, &heard, now);
        let requested_revision = inner.requested_revision;
        // Enforcement happens OUTSIDE the lock: spawning a process while
        // holding the state would stall every note_pane behind execvp.
        inner.active = wanted;
        inner.applied_revision = requested_revision;
        drop(inner);
        nudge.notify_all();

        if wanted {
            // A child that died on its own (caffeinate killed by something,
            // systemd-inhibit absent) is noticed here and retried after the
            // measured pause — a hot respawn loop would be worse than sleep.
            let dead = child
                .as_mut()
                .is_none_or(|held| !matches!(held.try_wait(), Ok(None)));
            if dead {
                release_assertion(&mut child);
                if retry_not_before.is_none_or(|at| now >= at) {
                    child = start_assertion();
                    retry_not_before = Some(now + RETRY_AFTER);
                }
            } else {
                retry_not_before = None;
            }
        } else {
            release_assertion(&mut child);
            retry_not_before = None;
        }
        #[cfg(windows)]
        windows_execution_state(wanted);

        let inner = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = nudge.wait_timeout_while(inner, KEEPER_TICK, |inner| {
            !inner.stop && inner.requested_revision == requested_revision
        });
    }
}

/// The hold itself, built but not started, so what gets spawned can be read
/// without a process table. `watch` is the pid the child's own life is tied
/// to — the window's, everywhere but the test that kills a stand-in.
#[cfg(target_os = "macos")]
fn assertion_command(watch: u32) -> std::process::Command {
    // -d display, -i idle, -s system-on-AC: the union of Orca's Electron
    // display blocker and its `caffeinate -i -s` child. -w <pid>: hold only
    // as long as that process lives, which is what makes the child die on
    // every road out of the window and not just the orderly one.
    let mut command = crate::proc::quiet_command("/usr/bin/caffeinate");
    command
        .args(["-d", "-i", "-s", "-w"])
        .arg(watch.to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    command
}

/// The platform's own "stay up" process, or `None` where none exists or it
/// could not start — the keeper retries on its own clock.
#[cfg(target_os = "macos")]
fn start_assertion() -> Option<std::process::Child> {
    assertion_command(std::process::id()).spawn().ok()
}

// The same question macOS answers with `-w` is open here: `systemd-inhibit`
// holds the lock through an fd owned by the process it runs, so an orphaned
// one keeps the lid inhibited with nobody left to kill it, and it has no
// `-w`. Closing it would take a watcher this file does not have. t-5444's
// measurement is macOS only, so this stays written down rather than guessed.
#[cfg(target_os = "linux")]
fn start_assertion() -> Option<std::process::Child> {
    crate::proc::quiet_command("systemd-inhibit")
        .args([
            "--what=sleep:handle-lid-switch",
            "--who=ZeroCode",
            "--why=Agents are working",
            "--mode=block",
            "sleep",
            "infinity",
        ])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn start_assertion() -> Option<std::process::Child> {
    None
}

fn release_assertion(child: &mut Option<std::process::Child>) {
    if let Some(mut held) = child.take() {
        let _ = held.kill();
        let _ = held.wait();
    }
}

/// `ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED` while agents
/// work, bare `ES_CONTINUOUS` to hand the machine back. Must run on the
/// keeper thread and only there: the state binds to the calling thread.
///
/// macOS's orphan question does not arise here: there is no child to outlive
/// anything, and a thread's execution state dies with the process that owns
/// it whichever way that process goes.
#[cfg(windows)]
fn windows_execution_state(active: bool) {
    use windows_sys::Win32::System::Power::{
        ES_CONTINUOUS, ES_DISPLAY_REQUIRED, ES_SYSTEM_REQUIRED, SetThreadExecutionState,
    };
    let state = if active {
        ES_CONTINUOUS | ES_SYSTEM_REQUIRED | ES_DISPLAY_REQUIRED
    } else {
        ES_CONTINUOUS
    };
    // SAFETY: a plain state-flag syscall on this thread; no memory crosses.
    unsafe {
        SetThreadExecutionState(state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The decision table, pure: Orca's three modes against the two-hour
    /// staleness horizon.
    #[test]
    fn the_machine_is_held_exactly_when_the_mode_and_the_clock_agree() {
        let now = Instant::now();
        let fresh = now - Duration::from_secs(60);
        let stale = now - STALE_AFTER - Duration::from_secs(1);
        let at_the_edge = now - STALE_AFTER;

        assert!(should_block(ComputerAwakeMode::On, &[], now));
        assert!(!should_block(ComputerAwakeMode::Off, &[fresh], now));
        assert!(should_block(ComputerAwakeMode::Auto, &[fresh], now));
        assert!(
            !should_block(ComputerAwakeMode::Auto, &[], now),
            "auto with nobody working held the machine"
        );
        assert!(
            !should_block(ComputerAwakeMode::Auto, &[stale], now),
            "a wedged pane older than the horizon still held the lid open"
        );
        // The boundary is inclusive, as `<=` reads in the measurement.
        assert!(should_block(ComputerAwakeMode::Auto, &[at_the_edge], now));
        // One fresh voice among the stale is enough.
        assert!(should_block(ComputerAwakeMode::Auto, &[stale, fresh], now));
    }

    /// The service's ledger: working marks a row, anything else clears it,
    /// and each `status` observation includes every preceding write without
    /// guessing how long the keeper thread needs to be scheduled.
    #[test]
    fn panes_come_and_go_and_the_status_follows() {
        let service = AwakeService::default();

        assert_eq!(service.status().0, ComputerAwakeMode::Off);
        service.note_pane(7, true);
        assert!(
            !service.status().1,
            "off held the machine because somebody worked"
        );

        service.set_mode(ComputerAwakeMode::Auto);
        assert!(service.status().1, "auto ignored a working pane");

        service.note_pane(7, false);
        assert!(!service.status().1, "a pane that stopped kept its hold");

        service.note_pane(9, true);
        service.forget_pane(9);
        assert!(!service.status().1, "a forgotten pane kept its hold");

        service.set_mode(ComputerAwakeMode::On);
        assert!(service.status().1, "on did not hold the machine");
    }

    /// The hold names the window it belongs to. `caffeinate -w <pid>` ends
    /// when that pid does, so every road out of the window — a clean exit,
    /// `std::process::exit`, a crash, SIGKILL, Tauri's own exit — takes the
    /// child with it. `Drop` is the fast release of an orderly exit, never
    /// the only one: measured 2026-09-20, 83 children had outlived their
    /// windows and hung off launchd, the oldest eight days old, and the
    /// machine had not slept for days.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_hold_names_the_window_it_belongs_to() {
        let command = assertion_command(std::process::id());
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        let watch = args
            .iter()
            .position(|arg| arg == "-w")
            .expect("the hold was spawned without -w, so it outlives the window");
        assert_eq!(
            args.get(watch + 1),
            Some(&std::process::id().to_string()),
            "-w did not name this process"
        );
        // The hold itself is unchanged: display, idle and system.
        for flag in ["-d", "-i", "-s"] {
            assert!(args.iter().any(|arg| arg == flag), "{flag} was dropped");
        }
    }

    /// And it really does let go. A stand-in for the window is SIGKILLed —
    /// the one road `Drop` cannot travel — and the child the keeper would
    /// have spawned ends on its own, with nobody to kill it.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_hold_lets_go_when_the_window_it_watches_is_killed() {
        let mut window = crate::proc::quiet_command("/bin/sleep")
            .arg("60")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("a stand-in for the window");
        let mut held = assertion_command(window.id()).spawn().expect("the hold");

        window.kill().expect("SIGKILL the stand-in window");
        let _ = window.wait();

        // Measured 2026-09-21: 11 / 16 / 37 ms across three trials. Only a
        // child that never lets go can spend this budget.
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut released = false;
        while !released && Instant::now() < deadline {
            released = matches!(held.try_wait(), Ok(Some(_)));
            if !released {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        if !released {
            // Never leave behind the very orphan this test is about.
            let _ = held.kill();
        }
        let _ = held.wait();

        assert!(
            released,
            "the hold outlived the process it watches — a killed window leaves an orphan"
        );
    }

    /// The wire spellings are Orca's own three words.
    #[test]
    fn the_mode_speaks_kebab_on_the_wire() {
        for (mode, word) in [
            (ComputerAwakeMode::On, "\"on\""),
            (ComputerAwakeMode::Off, "\"off\""),
            (ComputerAwakeMode::Auto, "\"auto\""),
        ] {
            assert_eq!(serde_json::to_string(&mode).expect("serialize"), word);
            assert_eq!(
                serde_json::from_str::<ComputerAwakeMode>(word).expect("deserialize"),
                mode
            );
        }
        assert!(serde_json::from_str::<ComputerAwakeMode>("\"always\"").is_err());
        assert_eq!(ComputerAwakeMode::default(), ComputerAwakeMode::Off);
    }
}
