//! The one process a phone's runner is: `xcodebuild test-without-building`, and
//! what keeps it honest without asking it anything.
//!
//! - Nothing polls. A reader watches the process's own output for the runner's
//!   address line, a waiter blocks on its exit, and a keeper sleeps until the one
//!   moment something is due: the idle stop, or a restart's turn.
//! - A window that dies leaves nothing behind. `xcodebuild` runs under a small shell
//!   that holds the read end of a pipe from this process; the pipe ends with the
//!   window, however the window ended, and the shell then ends `xcodebuild`.
//!   Stopping a runner on purpose takes the same road: closing the pipe.
//! - Nobody using a phone stops the runner after [`Life::idle`], unless the person
//!   turned unattended use on. Then a runner that dies comes back on its own, with
//!   a growing wait and a ceiling; a call that needs it does not wait for that turn.
//! - No start builds, signs or asks anything of a person: it runs what stands.

use std::io::{BufRead as _, BufReader};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use zerocode_core::agent_emulator::real_phone::{
    RESTART_FORGIVEN_SECS, RunnerFailure, SERVER_URL_BEGIN as READY_MARK, announced_base,
    runner_failure,
};

/// How much of a run's output is kept to read why it ended. A run writes about
/// 700 KB in its first minute (2026-11-03), nearly all of it each event's log.
const TAIL_BYTES: usize = 16 * 1024;

/// How long a stop waits for `xcodebuild` before the next start: it asks the phone
/// to end its test on TERM, and the shell sends KILL after five seconds.
const STOP_WAIT: Duration = Duration::from_secs(7);

/// The small shell. It keeps the pipe on descriptor 3 away from `xcodebuild` and
/// reads it in the background, where a shell would hand a plain job `/dev/null`.
const WATCHED: &str = r#"exec 3<&0
"$@" </dev/null 3<&- 2>&1 &
pid=$!
( cat <&3 >/dev/null 2>&1; kill -TERM "$pid" 2>/dev/null; sleep 5; kill -KILL "$pid" 2>/dev/null ) >/dev/null 2>&1 &
watch=$!
wait "$pid"
code=$?
kill "$watch" 2>/dev/null
exit $code
"#;

/// `program args…` under [`WATCHED`], with the pipes [`Life`] takes.
pub(super) fn watched(program: &str, args: &[String]) -> Command {
    let mut command = crate::proc::quiet_command("/bin/sh");
    command
        .arg("-c")
        .arg(WATCHED)
        .arg("zerocode-phone-runner")
        .arg(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

pub(super) type Spawn = Arc<dyn Fn() -> std::io::Result<Child> + Send + Sync>;
pub(super) type Unattended = Arc<dyn Fn() -> bool + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Phase {
    Down,
    Starting,
    Up,
    /// Its pipe is closed and the process is on its way out.
    Stopping,
    /// It died unattended and waits for its next try.
    Waiting,
    /// It died unattended more times than the ceiling.
    GaveUp,
}

impl Phase {
    pub(super) const fn word(self) -> &'static str {
        match self {
            Self::Down => "down",
            Self::Starting => "starting",
            Self::Up => "up",
            Self::Stopping => "stopping",
            Self::Waiting => "restarting",
            Self::GaveUp => "gave up",
        }
    }
}

struct State {
    phase: Phase,
    generation: u64,
    stdin: Option<ChildStdin>,
    tail: String,
    /// The HTTP base this generation printed on its ready line: the only address
    /// a call goes to, so a call never reaches another runner on the same port.
    announced: Option<String>,
    failure: Option<RunnerFailure>,
    last_used: Instant,
    up_since: Option<Instant>,
    retry_at: Option<Instant>,
    tries: usize,
    stopping: bool,
    keeper: bool,
    starts: u64,
}

pub(super) struct Life {
    spawn: Spawn,
    unattended: Unattended,
    idle: Duration,
    backoff: Vec<Duration>,
    state: Mutex<State>,
    changed: Condvar,
}

/// Why a runner is not up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NotUp {
    Failed(RunnerFailure),
    /// It was still starting when the wait ended.
    TimedOut,
    CouldNotStart,
    /// The Mac has no tunnel to the phone, so there is no one address to bind the
    /// runner to, and it is not started on every network instead.
    NoTunnel,
}

/// What a spawn says when it has no tunnel address: [`NotUp::NoTunnel`].
pub(super) const NO_TUNNEL: std::io::ErrorKind = std::io::ErrorKind::NotConnected;

impl Life {
    pub(super) fn new(
        spawn: Spawn,
        unattended: Unattended,
        idle: Duration,
        backoff: Vec<Duration>,
    ) -> Arc<Self> {
        Arc::new(Self {
            spawn,
            unattended,
            idle,
            backoff,
            state: Mutex::new(State {
                phase: Phase::Down,
                generation: 0,
                stdin: None,
                tail: String::new(),
                announced: None,
                failure: None,
                last_used: Instant::now(),
                up_since: None,
                retry_at: None,
                tries: 0,
                stopping: false,
                keeper: false,
                starts: 0,
            }),
            changed: Condvar::new(),
        })
    }

    fn held(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub(super) fn phase(&self) -> Phase {
        self.held().phase
    }

    /// The base the running generation printed, once it is up.
    pub(super) fn announced(&self) -> Option<String> {
        let state = self.held();
        (state.phase == Phase::Up)
            .then(|| state.announced.clone())
            .flatten()
    }

    #[cfg(test)]
    pub(super) fn starts(&self) -> u64 {
        self.held().starts
    }

    /// A use of the phone: the idle stop counts from here.
    pub(super) fn touch(&self) {
        self.held().last_used = Instant::now();
        self.changed.notify_all();
    }

    /// The runner, up: started now if it is not, however it came down. A call does
    /// not wait for a restart's turn, because the person may just have unlocked the
    /// phone or put the cable back. Blocks; this is `spawn_blocking`'s to call.
    pub(super) fn ensure_up(self: &Arc<Self>, wait: Duration) -> Result<(), NotUp> {
        let began = Instant::now();
        let mut state = self.held();
        state.last_used = began;
        let mut started = false;
        loop {
            match state.phase {
                Phase::Up => return Ok(()),
                Phase::Starting | Phase::Stopping => {}
                Phase::Down | Phase::Waiting | Phase::GaveUp if started => {
                    return Err(NotUp::Failed(state.failure.unwrap_or(RunnerFailure::Other)));
                }
                Phase::Down | Phase::Waiting | Phase::GaveUp => {
                    self.start(&mut state)?;
                    started = true;
                    continue;
                }
            }
            let Some(left) = wait.checked_sub(began.elapsed()) else {
                return Err(NotUp::TimedOut);
            };
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }

    fn start(self: &Arc<Self>, state: &mut MutexGuard<'_, State>) -> Result<(), NotUp> {
        let mut child = (self.spawn)().map_err(|error| {
            if error.kind() == NO_TUNNEL {
                NotUp::NoTunnel
            } else {
                NotUp::CouldNotStart
            }
        })?;
        state.generation += 1;
        state.starts += 1;
        state.phase = Phase::Starting;
        state.stopping = false;
        state.failure = None;
        state.retry_at = None;
        state.tail.clear();
        state.announced = None;
        state.stdin = child.stdin.take();
        let generation = state.generation;
        if let Some(out) = child.stdout.take() {
            let life = Arc::clone(self);
            let _ = std::thread::Builder::new()
                .name("phone-runner-output".to_string())
                .spawn(move || life.read_output(generation, out));
        }
        let life = Arc::clone(self);
        let _ = std::thread::Builder::new()
            .name("phone-runner-wait".to_string())
            .spawn(move || {
                let _ = child.wait();
                life.ended(generation);
            });
        if !state.keeper {
            state.keeper = true;
            let life = Arc::clone(self);
            let _ = std::thread::Builder::new()
                .name("phone-runner-keeper".to_string())
                .spawn(move || life.keep());
        }
        Ok(())
    }

    fn read_output(&self, generation: u64, out: impl std::io::Read) {
        let mut reader = BufReader::new(out);
        let mut line = Vec::new();
        loop {
            line.clear();
            match reader.read_until(b'\n', &mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let text = String::from_utf8_lossy(&line);
            let mut state = self.held();
            if state.generation != generation {
                return;
            }
            if text.contains(READY_MARK) && state.phase == Phase::Starting {
                state.announced = announced_base(&text);
                state.phase = Phase::Up;
                state.up_since = Some(Instant::now());
                state.last_used = Instant::now();
                drop(state);
                self.changed.notify_all();
                continue;
            }
            state.tail.push_str(&text);
            if state.tail.len() > TAIL_BYTES {
                let cut = state.tail.len() - TAIL_BYTES;
                let cut = (cut..state.tail.len())
                    .find(|at| state.tail.is_char_boundary(*at))
                    .unwrap_or(cut);
                state.tail.drain(..cut);
            }
        }
    }

    fn ended(&self, generation: u64) {
        let mut state = self.held();
        if state.generation != generation {
            return;
        }
        state.stdin = None;
        let stayed = state.up_since.take().map(|since| since.elapsed());
        if state.stopping {
            state.phase = Phase::Down;
        } else {
            state.failure = Some(runner_failure(&state.tail));
            if (self.unattended)() {
                if stayed.is_some_and(|up| up.as_secs() >= RESTART_FORGIVEN_SECS) {
                    state.tries = 0;
                }
                if let Some(wait) = self.backoff.get(state.tries) {
                    state.tries += 1;
                    state.retry_at = Some(Instant::now() + *wait);
                    state.phase = Phase::Waiting;
                } else {
                    state.phase = Phase::GaveUp;
                }
            } else {
                state.phase = Phase::Down;
            }
        }
        drop(state);
        self.changed.notify_all();
    }

    /// Asleep until the next moment something is due.
    fn keep(self: Arc<Self>) {
        let mut state = self.held();
        loop {
            let now = Instant::now();
            let due = match state.phase {
                Phase::Up if (self.unattended)() => {
                    // The switch may be turned off while it stands: one read per
                    // idle period is all it takes to hear that.
                    Some(self.idle)
                }
                Phase::Up => match self.idle.checked_sub(now - state.last_used) {
                    Some(left) if !left.is_zero() => Some(left),
                    _ => {
                        state.stopping = true;
                        state.stdin = None;
                        state.phase = Phase::Stopping;
                        None
                    }
                },
                Phase::Waiting => match state.retry_at {
                    Some(at) if at > now => Some(at - now),
                    _ => {
                        if !(self.unattended)() || self.start(&mut state).is_err() {
                            state.phase = Phase::Down;
                        }
                        continue;
                    }
                },
                Phase::Down | Phase::Starting | Phase::Stopping | Phase::GaveUp => None,
            };
            state = match due {
                Some(due) => {
                    self.changed
                        .wait_timeout(state, due)
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .0
                }
                None => self
                    .changed
                    .wait(state)
                    .unwrap_or_else(std::sync::PoisonError::into_inner),
            };
        }
    }

    /// End the runner by closing its pipe. Never blocks: it is called while the
    /// window exits.
    pub(super) fn stop(&self) {
        let mut state = self.held();
        state.stopping = true;
        state.retry_at = None;
        state.tries = 0;
        state.stdin = None;
        state.phase = match state.phase {
            Phase::Waiting | Phase::GaveUp | Phase::Down => Phase::Down,
            Phase::Starting | Phase::Up | Phase::Stopping => Phase::Stopping,
        };
        drop(state);
        self.changed.notify_all();
    }

    /// End it and wait until it is gone, for a start that follows.
    pub(super) fn stop_and_wait(&self) {
        self.stop();
        let began = Instant::now();
        let mut state = self.held();
        while !matches!(state.phase, Phase::Down) {
            let Some(left) = STOP_WAIT.checked_sub(began.elapsed()) else {
                return;
            };
            state = self
                .changed
                .wait_timeout(state, left)
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .0;
        }
    }
}
