//! The macOS session: a signed helper app over an authenticated UNIX socket.
//!
//! A signed helper owns macOS TCC permissions and the accessibility snapshot
//! cache. The ZeroCode window owns one authenticated socket to that helper;
//! agent shims send typed commands to the window rather than launching fresh
//! helpers, so element indexes remain meaningful between observation and
//! action.
//!
//! The helper is launched through LaunchServices (`open -n`), never spawned as
//! this process's child. TCC judges a privacy call by the *responsible*
//! process, and a child spawned straight from the window is judged as the
//! window: every Accessibility and Screen Recording read ran against
//! ZeroCode.app's grants while the permission probe — which `open` always
//! launched — read the helper bundle's own, granted, state. Measured 2026-09-03
//! with the same helper binary: spawned directly it reported both permissions
//! `not-granted`; through `open -n` both `granted`. That gap was the settings
//! page saying 「준비됨 · 허용됨」 above an agent whose every capture came back
//! `permission_denied`.

use std::io::{BufRead as _, Read as _, Write as _};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{Value, json};
use zerocode_core::computer_use::{
    COMPUTER_USE_PROTOCOL_VERSION, ComputerPermissionId, ComputerPermissionReport,
    ComputerPermissionReset, ComputerPermissionRow, ComputerPermissionRowAction,
    ComputerPermissionSetup, ComputerPermissionState, ComputerPermissionStatus,
    ComputerPermissionTccRow, ComputerPermissionUnreadable,
};
use zerocode_core::computer_use_protocol::error_code;

use super::ComputerUseError;
use super::permissions::missing_permissions;
use super::session::{ProviderSession, SessionFailure};

mod panel;
mod tcc;

#[cfg(test)]
mod background_tests;
#[cfg(test)]
mod parallel_bench;
#[cfg(test)]
pub(crate) mod stand_in;
#[cfg(test)]
mod twin_bench;

const HELPER_APP_NAME: &str = "ZeroCode Computer Use.app";
const HELPER_EXECUTABLE: &str = "zerocode-computer-use-macos";
const HELPER_BUNDLE_ID: &str = "dev.zerocode.app.computer-use";
/// The window's own identity — the responsible process TCC judges for Screen
/// Recording. One spelling, the lane's (app_paths pins it to tauri.conf.json).
const APP_BUNDLE_ID: &str = zerocode_lane::APP_IDENTIFIER;
const APP_NAME: &str = "ZeroCode";
const HELPER_NAME: &str = "ZeroCode Computer Use";
const PERMISSION_PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const PERMISSION_POLL_INTERVAL: Duration = Duration::from_millis(100);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const MAX_RESPONSE_BYTES: u64 = 64 * 1024 * 1024;
/// How long the window's waiting request (t-66753 B) waits for the helper's
/// stop to change before it is asked again: well inside the read limit every
/// answer has (`REQUEST_TIMEOUT`), so a quiet helper is asked again rather
/// than taken for a dead one.
const STOP_WATCH_WAIT: Duration = Duration::from_secs(20);
/// The helper's road for that request (`OperatorGuardHost.watch`).
const STOP_WATCH_METHOD: &str = "stopWatch";
/// How many unused connections to keep. More than this at once is a burst, and
/// the extra ones are closed when it is over.
const MAX_IDLE_WIRES: usize = 8;
/// How long a connection the helper's queue refused is tried again, and the
/// pause between two tries. The helper's queue holds eight connections it has
/// not taken yet (`listen(socketFd, 8)`), and the groups of one `parallel`
/// connect in the same instant: past the queue the system refuses at once.
/// The helper takes a connection within moments, so a refusal while it lives
/// is a queue that was full for one.
const QUEUE_FULL_WAIT: Duration = Duration::from_secs(2);
const QUEUE_FULL_PAUSE: Duration = Duration::from_millis(2);
/// A living helper that has taken no connection for this long, whatever asked,
/// is no busy helper: the time a helper is given to listen when it starts.
/// The session goes then, and its helper is sent away before another starts.
const UNTAKEN_IS_BROKEN: Duration = CONNECT_TIMEOUT;

/// Since when a living helper has taken no connection, across the requests
/// that asked for one.
#[derive(Default)]
struct Untaken(Option<std::time::Instant>);

impl Untaken {
    /// A connection asked for at `asked` was refused to the last, at `now`.
    /// Whether the helper has now taken none for `patience`.
    fn refused(
        &mut self,
        asked: std::time::Instant,
        now: std::time::Instant,
        patience: Duration,
    ) -> bool {
        now.duration_since(*self.0.get_or_insert(asked)) >= patience
    }

    /// The helper took a connection.
    fn taken(&mut self) {
        self.0 = None;
    }
}

/// Why a request got no connection to the helper.
enum NoWire {
    /// The helper lives and took no new connection through the wait. Nothing
    /// was sent, and the session stands: this request alone is refused.
    Busy,
    /// The road is broken — the helper is gone, the socket is, or the helper
    /// has taken no connection for [`UNTAKEN_IS_BROKEN`]. The session is
    /// discarded, its helper sent away, and the next call starts a new one.
    Broken(ComputerUseError),
}
// Every waiting request is answered inside the limit every answer has.
const _: () = assert!(STOP_WATCH_WAIT.as_secs() < REQUEST_TIMEOUT.as_secs());

pub(super) struct Session {
    /// Connections with no request on them. A request takes one or opens a new
    /// one, so requests to different apps run at once and the helper keeps them
    /// in order per app.
    idle: std::sync::Mutex<Vec<Wire>>,
    /// Since when the helper has taken no new connection, if it has not.
    untaken: std::sync::Mutex<Untaken>,
    socket: PathBuf,
    token: String,
    /// The window's waiting request (t-66753 B): its own connection, shut
    /// when the session ends, and the thread that waits on it.
    watching: Option<StopWatch>,
    /// The helper's pid, read off the socket once it answered
    /// (`LOCAL_PEERPID`). Not a `Child`: LaunchServices launched it, so it is
    /// nobody's child here — see the module comment.
    helper_pid: Option<libc::pid_t>,
    directory: PathBuf,
    supports_background: bool,
}

/// What a launch leaves for the connection (t-66753 C): the helper's private
/// folder, the socket it listens on, the token file it reads at start and
/// the token it expects on every request.
pub(crate) struct Launched {
    pub(crate) directory: PathBuf,
    pub(crate) socket: PathBuf,
    pub(crate) token_file: PathBuf,
    pub(crate) token: String,
}

/// One authenticated connection to the helper: a request goes out as one
/// line carrying the token, its answer comes back as one line, each within
/// `REQUEST_TIMEOUT`.
struct Wire {
    stream: std::os::unix::net::UnixStream,
    reader: std::io::BufReader<std::os::unix::net::UnixStream>,
    token: String,
    next_id: u64,
}

impl Wire {
    fn new(
        stream: std::os::unix::net::UnixStream,
        token: String,
    ) -> Result<Self, ComputerUseError> {
        stream
            .set_read_timeout(Some(REQUEST_TIMEOUT))
            .map_err(io_error("set helper read timeout"))?;
        stream
            .set_write_timeout(Some(REQUEST_TIMEOUT))
            .map_err(io_error("set helper write timeout"))?;
        let reader = std::io::BufReader::new(
            stream
                .try_clone()
                .map_err(io_error("clone helper socket"))?,
        );
        Ok(Self {
            stream,
            reader,
            token,
            next_id: 1,
        })
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, SessionFailure> {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        let line = serde_json::to_vec(&json!({
            "id": id,
            "method": method,
            "params": params,
            "token": self.token,
        }))
        .map_err(|error| {
            SessionFailure::Provider(ComputerUseError::invalid_argument(error.to_string()))
        })?;
        self.stream
            .write_all(&line)
            .and_then(|()| self.stream.write_all(b"\n"))
            .and_then(|()| self.stream.flush())
            .map_err(io_error("write Computer Use request"))
            .map_err(SessionFailure::Transport)?;

        let mut response = Vec::new();
        let read = self
            .reader
            .by_ref()
            .take(MAX_RESPONSE_BYTES + 1)
            .read_until(b'\n', &mut response)
            .map_err(io_error("read Computer Use response"))
            .map_err(SessionFailure::Transport)?;
        if read == 0 {
            return Err(SessionFailure::Transport(ComputerUseError::new(
                error_code::ACCESSIBILITY_ERROR,
                "Computer Use helper closed the connection",
            )));
        }
        if response.len() as u64 > MAX_RESPONSE_BYTES {
            return Err(SessionFailure::Transport(ComputerUseError::new(
                error_code::ACCESSIBILITY_ERROR,
                "Computer Use response exceeded the 64 MiB safety cap",
            )));
        }
        let mut response: Value = serde_json::from_slice(&response).map_err(|error| {
            SessionFailure::Transport(ComputerUseError::new(
                error_code::ACCESSIBILITY_ERROR,
                error.to_string(),
            ))
        })?;
        if response.get("id").and_then(Value::as_u64) != Some(id) {
            return Err(SessionFailure::Transport(ComputerUseError::new(
                error_code::ACCESSIBILITY_ERROR,
                "Computer Use helper returned a mismatched request id",
            )));
        }
        if response.get("ok").and_then(Value::as_bool) == Some(true) {
            // Taken, not cloned: an answer can carry a picture of several MB.
            return Ok(response.get_mut("result").map_or(Value::Null, Value::take));
        }
        let error = response.get("error").unwrap_or(&Value::Null);
        Err(SessionFailure::Provider(
            ComputerUseError::new(
                error
                    .get("code")
                    .and_then(Value::as_str)
                    .unwrap_or(error_code::ACCESSIBILITY_ERROR),
                error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Computer Use helper refused the request"),
            )
            .with_delivery(error.get("delivery").and_then(Value::as_str)),
        ))
    }
}

/// A separate authenticated connection carries panel updates. It never takes
/// the provider/session lock, so taking control can cancel an in-flight act.
#[derive(Clone, PartialEq, Eq)]
struct PanelEndpoint {
    socket: PathBuf,
    token: String,
}

static PANEL_ENDPOINT: std::sync::Mutex<Option<PanelEndpoint>> = std::sync::Mutex::new(None);

fn remember_panel(socket: &Path, token: &str) {
    *PANEL_ENDPOINT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(PanelEndpoint {
        socket: socket.to_path_buf(),
        token: token.to_string(),
    });
}

fn forget_panel(socket: &Path) {
    let mut held = PANEL_ENDPOINT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if held
        .as_ref()
        .is_some_and(|endpoint| endpoint.socket == socket)
    {
        *held = None;
        drop(held);
        sync_share_panel(&super::share::state());
    }
}

pub(super) fn sync_share_panel(state: &Value) {
    let endpoint = PANEL_ENDPOINT
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone();
    panel::publish(state, endpoint);
}

/// The connection the window's waiting request rides, and its thread.
struct StopWatch {
    stream: std::os::unix::net::UnixStream,
    thread: std::thread::JoinHandle<()>,
    /// Set before the connection is shut on purpose: the request that ends
    /// because of it is no loss, and says nothing.
    leaving: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Set by the watch thread as it ends, for whatever reason: the panel
    /// worker reads it to know its helper connection is gone (t-75398).
    ended: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl StopWatch {
    /// One waiting request to the helper at a time, on a connection of its
    /// own: never the session's lock, so it waits behind no request in
    /// flight and none waits behind it (t-62469 B2). Each answer goes to the
    /// guard, which takes in the person's chord at once; then it asks again
    /// after the stop it heard. A helper that cannot answer one — or a
    /// connection that broke — ends it, and the end is said to the guard
    /// (t-68324 ①): the session goes on without it, the person's chord
    /// reaching the window only at its next command. One that never began is
    /// said the same way.
    fn start(socket: &Path, token: &str) -> Option<Self> {
        match Self::begin(socket, token) {
            Ok(watch) => Some(watch),
            Err(why) => {
                super::guard::watch_lost(&format!("the waiting request could not start: {why}"));
                // No watch thread exists to withdraw this endpoint. Wake the
                // panel worker after the guard has recorded the lost watch.
                forget_panel(socket);
                None
            }
        }
    }

    fn begin(socket: &Path, token: &str) -> Result<Self, String> {
        let stream = std::os::unix::net::UnixStream::connect(socket)
            .map_err(|error| format!("connect: {error}"))?;
        let kept = stream
            .try_clone()
            .map_err(|error| format!("clone: {error}"))?;
        let mut wire = Wire::new(stream, token.to_string()).map_err(|error| error.message)?;
        let wait = u64::try_from(STOP_WATCH_WAIT.as_millis()).unwrap_or(u64::MAX);
        let leaving = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let leaving_seen = std::sync::Arc::clone(&leaving);
        let lost = move |reason: String| {
            if !leaving_seen.load(std::sync::atomic::Ordering::Acquire) {
                super::guard::watch_lost(&reason);
            }
        };
        // Begun before the thread can end, so an end is never overwritten.
        super::guard::watch_began();
        let socket = socket.to_path_buf();
        let ended = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_ended = std::sync::Arc::clone(&ended);
        let thread = std::thread::Builder::new()
            .name("computer-use-stop-watch".into())
            .spawn(move || {
                let mut after: Option<u64> = None;
                let mut after_share: Option<i64> = None;
                loop {
                    let mut asked = serde_json::Map::new();
                    asked.insert("ms".into(), wait.into());
                    if let Some(after) = after {
                        asked.insert("after".into(), after.into());
                    }
                    if let Some(event) = after_share {
                        asked.insert("afterShare".into(), event.into());
                    }
                    let heard = match wire.request(STOP_WATCH_METHOD, Value::Object(asked)) {
                        Ok(heard) => heard,
                        Err(failure) => {
                            let error = failure.into_error_pub();
                            lost(format!(
                                "the waiting request failed: {} ({})",
                                error.message, error.code
                            ));
                            break;
                        }
                    };
                    super::guard::hear_watched(&heard);
                    super::share::hear_panel(&heard);
                    after_share = heard.pointer("/shareControl/event").and_then(Value::as_i64);
                    // An answer with no count to ask after is not this road's.
                    let Some(revision) = heard.get("stopRevision").and_then(Value::as_u64) else {
                        lost("the helper's answer carried no stop count to ask after".to_string());
                        break;
                    };
                    after = Some(revision);
                }
                // Publish liveness before forgetting the endpoint: that
                // presentation can wake the panel worker before we return.
                worker_ended.store(true, std::sync::atomic::Ordering::Release);
                forget_panel(&socket);
            })
            .map_err(|error| format!("thread: {error}"))?;
        Ok(Self {
            stream: kept,
            thread,
            leaving,
            ended,
        })
    }

    /// Shut its connection, which ends the request waiting on it, and wait
    /// for its thread to end: nothing it heard is taken in after the session.
    fn end(self) {
        self.leaving
            .store(true, std::sync::atomic::Ordering::Release);
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
        let _ = self.thread.join();
    }
}

impl ProviderSession for Session {
    fn supports_background(&self) -> bool {
        self.supports_background
    }
    fn helper_pid(&self) -> Option<i32> {
        self.helper_pid
    }

    fn start() -> Result<Self, ComputerUseError> {
        Self::connect(launch()?)
    }

    fn dismiss(&self) {
        self.dismiss_helper();
    }

    fn request(&self, method: &str, params: Value) -> Result<Value, SessionFailure> {
        let mut wire = match self.take_wire(QUEUE_FULL_WAIT) {
            Ok(wire) => wire,
            // No second helper beside one that lives: it may still carry
            // another group's steps, and the window's stop reaches one helper.
            Err(NoWire::Busy) => return Err(SessionFailure::Provider(helper_took_no_connection())),
            Err(NoWire::Broken(error)) => return Err(SessionFailure::Transport(error)),
        };
        let answer = wire.request(method, params);
        // A wire that broke, or whose reply no longer lines up, is not reused.
        if !matches!(answer, Err(SessionFailure::Transport(_))) {
            let mut idle = self
                .idle
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if idle.len() < MAX_IDLE_WIRES {
                idle.push(wire);
            }
        }
        answer
    }
}

/// Launch the signed helper through LaunchServices with a fresh token —
/// the first half of [`Session::start`]; [`Session::connect`] is the other.
fn launch() -> Result<Launched, ComputerUseError> {
    // The bundle, because LaunchServices launches bundles; the executable
    // check keeps the older "app found but hollow" answer.
    let app = helper_app_path()
        .filter(|_| helper_executable_path().is_some())
        .ok_or_else(|| {
            ComputerUseError::new(
                error_code::ACCESSIBILITY_ERROR,
                "ZeroCode Computer Use.app was not found",
            )
        })?;
    launch_at(&app)
}

/// Launch the helper bundle at `app`.
fn launch_at(app: &Path) -> Result<Launched, ComputerUseError> {
    use std::os::unix::fs::PermissionsExt as _;

    let nonce = crate::hooks::random_token().ok_or_else(|| {
        ComputerUseError::new(
            error_code::ACCESSIBILITY_ERROR,
            "could not mint a helper token",
        )
    })?;
    let directory = PathBuf::from(format!(
        "/tmp/zerocode-computer-use-{}-{}",
        std::process::id(),
        &nonce[..12]
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).map_err(io_error("remove stale helper state"))?;
    }
    std::fs::create_dir(&directory).map_err(io_error("create helper state"))?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .map_err(io_error("protect helper state"))?;
    let socket = directory.join("provider.sock");
    let token_file = directory.join("provider.token");
    std::fs::write(&token_file, &nonce).map_err(io_error("write helper token"))?;
    std::fs::set_permissions(&token_file, std::fs::Permissions::from_mode(0o600))
        .map_err(io_error("protect helper token"))?;

    // `open -n`: a new instance, launched by LaunchServices so that TCC
    // sees the helper bundle and not this window (module comment). The
    // environment does not cross that launch, so the debug-only peer
    // fallback travels as `--env`; the per-launch 0600 token still
    // authenticates the exact socket session either way.
    let mut command = crate::proc::quiet_command("/usr/bin/open");
    command.arg("-n").arg(app);
    if cfg!(debug_assertions) {
        command.args(["--env", "ZEROCODE_COMPUTER_USE_ALLOW_UNBUNDLED=1"]);
    }
    // The guard's table — the pace and the session count — travels the
    // same way: the helper keeps no numbers of its own.
    command
        .arg("--env")
        .arg(guard_table_env())
        .arg("--args")
        .args(["--agent", &socket.to_string_lossy(), "--token-file"])
        .arg(&token_file)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null());
    let launched = command
        .output()
        .map_err(io_error("launch Computer Use helper"))?;
    if !launched.status.success() {
        cleanup(&directory);
        let why = String::from_utf8_lossy(&launched.stderr).trim().to_string();
        return Err(ComputerUseError::new(
            error_code::ACCESSIBILITY_ERROR,
            format!("LaunchServices could not launch the Computer Use helper: {why}"),
        ));
    }
    Ok(Launched {
        directory,
        socket,
        token_file,
        token: nonce,
    })
}

impl Session {
    /// A connection with no request on it: an idle one, or a new one — tried
    /// again through `wait` while the helper's queue is full.
    fn take_wire(&self, wait: Duration) -> Result<Wire, NoWire> {
        if let Some(wire) = self
            .idle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .pop()
        {
            return Ok(wire);
        }
        let asked = std::time::Instant::now();
        let untaken = || {
            self.untaken
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        };
        match connect_past_a_full_queue(&self.socket, wait, || self.helper_lives()) {
            Ok(stream) => {
                untaken().taken();
                Wire::new(stream, self.token.clone()).map_err(NoWire::Broken)
            }
            Err(error)
                if error.kind() == std::io::ErrorKind::ConnectionRefused
                    && self.helper_lives()
                    && !untaken().refused(asked, std::time::Instant::now(), UNTAKEN_IS_BROKEN) =>
            {
                Err(NoWire::Busy)
            }
            Err(error) => Err(NoWire::Broken(io_error(
                "connect to the Computer Use helper",
            )(error))),
        }
    }

    /// Ask the helper to go, and signal it if it stays: at the session's end,
    /// and as soon as the window discards the session — a request of another
    /// group may still hold it, and a helper left running then would go on
    /// with that group's steps out of reach of the window's stop, which is
    /// aimed at the session that stands (t-109578). Asked twice it is gone
    /// the second time, and nothing is sent or signalled.
    fn dismiss_helper(&self) {
        // Its waiting request ends with it, on purpose: that is no loss.
        if let Some(watching) = &self.watching {
            watching
                .leaving
                .store(true, std::sync::atomic::Ordering::Release);
        }
        // A full queue is not waited out for a goodbye.
        if let Ok(mut wire) = self.take_wire(Duration::ZERO)
            && let Ok(line) = serde_json::to_vec(&json!({
                "id": wire.next_id,
                "method": "terminate",
                "params": {},
                "token": self.token,
            }))
        {
            let _ = wire.stream.write_all(&line);
            let _ = wire.stream.write_all(b"\n");
            let _ = wire.stream.flush();
        }
        // `terminate` is the helper's own exit; the signal is for a helper
        // that stopped reading. Only ever aimed at the process this socket
        // answered from, and only while that pid still runs our executable —
        // a pid is reusable, a path is not.
        if let Some(pid) = self.helper_pid {
            let deadline = std::time::Instant::now() + Duration::from_millis(300);
            while helper_runs_at(pid) {
                if std::time::Instant::now() >= deadline {
                    // SAFETY: `kill` with a positive pid and a plain signal
                    // number has no memory effects on this process.
                    let _ = unsafe { libc::kill(pid, libc::SIGTERM) };
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        }
    }

    /// Whether the helper is known to be there still: its pid lives and still
    /// runs the helper's executable — a pid is reusable, a path is not. A
    /// helper whose pid was never read is not known to live: its refusal is a
    /// broken road, as it always was.
    fn helper_lives(&self) -> bool {
        self.helper_pid.is_some_and(helper_stands_at)
    }

    pub(super) fn panel_watch_ended(&self) -> bool {
        self.watching
            .as_ref()
            .is_none_or(|watch| watch.ended.load(std::sync::atomic::Ordering::Acquire))
    }

    /// Connect to a launched helper and shake hands — the second half of
    /// [`Session::start`]: the pid read off the socket, the token on every
    /// request, the protocol and the guard table held to this window's.
    pub(crate) fn connect(launched: Launched) -> Result<Self, ComputerUseError> {
        let Launched {
            directory,
            socket,
            token_file,
            token,
        } = launched;
        // No child to watch for an early exit: a helper that dies before it
        // listens simply never answers, and the deadline below says so. A
        // helper that listens but is never claimed reaps itself after its own
        // thirty-second deadline (`AgentRuntime.unclaimedSessionDeadline`).
        let started = std::time::Instant::now();
        let stream = loop {
            match std::os::unix::net::UnixStream::connect(&socket) {
                Ok(stream) => break stream,
                Err(_error) if started.elapsed() < CONNECT_TIMEOUT => {
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(error) => {
                    cleanup(&directory);
                    return Err(ComputerUseError::new(
                        error_code::ACTION_TIMEOUT,
                        format!("timed out connecting to Computer Use helper: {error}"),
                    ));
                }
            }
        };
        let helper_pid = peer_pid(&stream);
        let _ = std::fs::remove_file(&token_file);
        let wire = Wire::new(stream, token.clone())?;
        let mut client = Self {
            idle: std::sync::Mutex::new(vec![wire]),
            untaken: std::sync::Mutex::new(Untaken::default()),
            socket: socket.clone(),
            token,
            watching: None,
            helper_pid,
            directory,
            supports_background: false,
        };
        let handshake = client
            .request("handshake", json!({}))
            .map_err(SessionFailure::into_error_pub)?;
        let protocol = handshake
            .get("protocolVersion")
            .and_then(Value::as_u64)
            .unwrap_or_default();
        if protocol != COMPUTER_USE_PROTOCOL_VERSION {
            return Err(ComputerUseError::new(
                error_code::PROVIDER_INCOMPATIBLE,
                format!(
                    "Computer Use helper protocol {protocol} is incompatible with required protocol {COMPUTER_USE_PROTOCOL_VERSION}"
                ),
            ));
        }
        validate_guard_budget(&handshake)?;
        client.supports_background =
            handshake.pointer("/supports/actions/background") == Some(&Value::Bool(true));
        if handshake.pointer("/supports/windows/shareControlPanel") == Some(&Value::Bool(true)) {
            remember_panel(&socket, &client.token);
            sync_share_panel(&super::share::state());
        }
        // The person's chord reaches the window as the helper hears it
        // (t-66753 B), not at the window's next command.
        client.watching = StopWatch::start(&socket, &client.token);
        Ok(client)
    }
}

/// The refusal of a request for which the living helper took no connection:
/// nothing of it was sent, and the same helper answers the next one.
fn helper_took_no_connection() -> ComputerUseError {
    ComputerUseError::new(
        error_code::ACTION_TIMEOUT,
        format!(
            "the Computer Use helper took no new connection for {} s; nothing was sent. It is the same helper still: try again in a moment",
            QUEUE_FULL_WAIT.as_secs()
        ),
    )
    .with_delivery(Some(zerocode_core::computer_use_protocol::NOT_DELIVERED))
}

/// Connect to the helper at `socket`, trying again while its queue is full.
/// A refused connection sent nothing. Taken for a broken road it discards the
/// session, and the next step starts a second helper beside one that still
/// carries the other groups' steps — out of reach of the window's stop, with
/// an app's turns and the session's count split between the two (t-109501).
/// Given up at once when `lives` says the helper is gone, and after `wait`
/// when it never took the connection.
fn connect_past_a_full_queue(
    socket: &Path,
    wait: Duration,
    lives: impl Fn() -> bool,
) -> std::io::Result<std::os::unix::net::UnixStream> {
    let started = std::time::Instant::now();
    loop {
        match std::os::unix::net::UnixStream::connect(socket) {
            Err(error)
                if error.kind() == std::io::ErrorKind::ConnectionRefused
                    && started.elapsed() < wait
                    && lives() =>
            {
                std::thread::sleep(QUEUE_FULL_PAUSE);
            }
            connected => return connected,
        }
    }
}

impl SessionFailure {
    fn into_error_pub(self) -> ComputerUseError {
        match self {
            Self::Transport(error) | Self::Provider(error) => error,
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        forget_panel(&self.directory.join("provider.sock"));
        if let Some(watching) = self.watching.take() {
            watching.end();
        }
        // The session is over, and its watch with it: nothing is lost.
        super::guard::watch_left();
        self.dismiss_helper();
        cleanup(&self.directory);
    }
}

/// The pid on the far end of a connected UNIX socket (`LOCAL_PEERPID`, the
/// same option the helper reads to authorize *this* side).
fn peer_pid(stream: &std::os::unix::net::UnixStream) -> Option<libc::pid_t> {
    use std::os::unix::io::AsRawFd as _;
    const LOCAL_PEERPID: libc::c_int = 2;
    let mut pid: libc::pid_t = 0;
    let mut length = std::mem::size_of::<libc::pid_t>() as libc::socklen_t;
    // SAFETY: `pid` and `length` outlive the call and `length` names `pid`'s
    // exact size; getsockopt writes at most that many bytes.
    let rc = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_LOCAL,
            LOCAL_PEERPID,
            (&raw mut pid).cast::<libc::c_void>(),
            &raw mut length,
        )
    };
    (rc == 0 && pid > 0).then_some(pid)
}

/// Whether the helper a session read off its socket is there still. In a
/// test the helper is a stand-in on a thread of the test's own process, whose
/// executable is no helper's (`stand_in`): that process stands for it.
fn helper_stands_at(pid: libc::pid_t) -> bool {
    #[cfg(test)]
    if u32::try_from(pid).is_ok_and(|pid| pid == std::process::id()) {
        return true;
    }
    helper_runs_at(pid)
}

/// Whether `pid` is alive *and* still runs the Computer Use helper executable.
fn helper_runs_at(pid: libc::pid_t) -> bool {
    let mut buffer = [0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: the buffer outlives the call and its length is passed with it;
    // proc_pidpath writes at most that many bytes.
    let written = unsafe {
        libc::proc_pidpath(
            pid,
            buffer.as_mut_ptr().cast::<libc::c_void>(),
            buffer.len() as u32,
        )
    };
    if written <= 0 {
        return false;
    }
    let path = String::from_utf8_lossy(&buffer[..written as usize]);
    path.ends_with(HELPER_EXECUTABLE)
}

fn validate_guard_budget(handshake: &Value) -> Result<(), ComputerUseError> {
    let expected = Value::Object(zerocode_core::computer_use::guard_table());
    let actual = handshake.pointer("/supports/desktop/guard/budget");
    if actual != Some(&expected) {
        return Err(ComputerUseError::new(
            error_code::PROVIDER_INCOMPATIBLE,
            format!(
                "Computer Use helper guard policy differs from this window: expected {expected}, got {}. Install matching app and helper builds.",
                actual.unwrap_or(&Value::Null)
            ),
        ));
    }
    Ok(())
}

/// The `open --env` word that hands the helper the core's guard table.
fn guard_table_env() -> String {
    format!(
        "{}={}",
        zerocode_core::computer_use::COMPUTER_GUARD_TABLE_ENV,
        Value::Object(zerocode_core::computer_use::guard_table())
    )
}

fn cleanup(path: &Path) {
    let _ = std::fs::remove_dir_all(path);
}

fn io_error(context: &'static str) -> impl FnOnce(std::io::Error) -> ComputerUseError {
    move |error| {
        ComputerUseError::new(
            error_code::ACCESSIBILITY_ERROR,
            format!("{context}: {error}"),
        )
    }
}

fn helper_app_path() -> Option<PathBuf> {
    if let Some(path) = std::env::var_os("ZEROCODE_COMPUTER_MACOS_HELPER_APP_PATH")
        .map(PathBuf::from)
        .filter(|path| path.is_dir())
    {
        return Some(path);
    }
    if let Some(path) = super::resource_dir()
        .map(|root| root.join(HELPER_APP_NAME))
        .filter(|path| path.is_dir())
    {
        return Some(path);
    }
    // A test build reaches the person's desktop only through a helper it
    // names (the variable above, as the live tests say): a unit test that
    // expects its press to be refused must never find the dev build instead.
    if cfg!(test) {
        return None;
    }
    let dev = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("native/computer-use-macos/.build/release")
        .join(HELPER_APP_NAME);
    dev.is_dir().then_some(dev)
}

fn helper_executable_path() -> Option<PathBuf> {
    let executable = helper_app_path()?
        .join("Contents/MacOS")
        .join(HELPER_EXECUTABLE);
    executable.is_file().then_some(executable)
}

fn helper_signing_identity() -> Option<zerocode_core::computer_use::ComputerSigningIdentity> {
    let output = crate::proc::quiet_command("/usr/bin/codesign")
        .args(["-d", "-r", "-", "--verbose=2"])
        .arg(helper_app_path()?)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let details = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    signing_identity(&details)
}

fn signing_identity(details: &str) -> Option<zerocode_core::computer_use::ComputerSigningIdentity> {
    use zerocode_core::computer_use::ComputerSigningIdentity;
    if details.contains("Signature=adhoc") {
        Some(ComputerSigningIdentity::Adhoc)
    } else if details.contains("certificate leaf =") {
        Some(ComputerSigningIdentity::Local)
    } else {
        None
    }
}

#[must_use]
pub(super) fn permission_status() -> ComputerPermissionReport {
    let report = match permission_status_macos() {
        Ok(report) => report,
        Err(error) => ComputerPermissionReport {
            identity: helper_signing_identity(),
            platform: "darwin".into(),
            helper_app_path: helper_app_path().map(|path| path.to_string_lossy().into_owned()),
            helper_unavailable_reason: Some(error.message),
            permissions: missing_permissions(),
            judged_rows: helper_app_path()
                .map(|helper| judged_rows(&helper))
                .unwrap_or_default(),
            tcc_rows: helper_app_path()
                .map(|helper| tcc_rows(&helper, None))
                .unwrap_or_default(),
        },
    };
    settle_session_for(&report.permissions);
    report
}

/// The rows as the window last read them: what a helper started since counts on.
static ROWS_READ: std::sync::Mutex<Option<Vec<(ComputerPermissionId, ComputerPermissionStatus)>>> =
    std::sync::Mutex::new(None);

/// A granted ScreenCapture row is only observed by a new helper process, so
/// a reading that moved a row ends the session, and the next call starts a
/// helper that sees it. A reading that moved none leaves the session and what
/// it has under way alone: the window reads the rows whenever it is focused,
/// and ending a session sends its helper away with every act it carries
/// (t-109675).
fn settle_session_for(rows: &[ComputerPermissionState]) {
    let moved = rows_moved(
        &mut ROWS_READ
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner),
        rows,
    );
    if moved {
        super::shutdown();
    }
}

/// Whether `rows` differ from the ones `read` last, which they replace. The
/// first reading has none to compare with and counts as moved: nothing says
/// what a helper that already stands started under.
fn rows_moved(
    read: &mut Option<Vec<(ComputerPermissionId, ComputerPermissionStatus)>>,
    rows: &[ComputerPermissionState],
) -> bool {
    let rows: Vec<_> = rows.iter().map(|row| (row.id, row.status)).collect();
    let moved = read.as_ref() != Some(&rows);
    *read = Some(rows);
    moved
}

/// Forget the last reading, so the next one counts as moved whatever it
/// says. A row changed outside a reading — it was reset, or the person was
/// sent to System Settings to set it — and a helper that starts before the
/// row is set again starts under the row as it stands in between: a reading
/// that then finds the rows as they were last read would leave that helper
/// standing, blind to a grant given after it started (t-109717).
fn forget_rows_read() {
    *ROWS_READ
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
}

fn permission_status_macos() -> Result<ComputerPermissionReport, ComputerUseError> {
    // A test says what its thread's probe reads, and no helper app starts.
    #[cfg(test)]
    if let Some(report) = stand_in::probe_report() {
        return Ok(report);
    }
    permission_probe(None).map(|(report, _)| report)
}

fn permission_probe(
    requested: Option<ComputerPermissionId>,
) -> Result<(ComputerPermissionReport, bool), ComputerUseError> {
    use std::os::unix::fs::PermissionsExt as _;

    let helper = helper_app_path().ok_or_else(|| {
        ComputerUseError::new(
            error_code::ACCESSIBILITY_ERROR,
            "ZeroCode Computer Use.app was not found",
        )
    })?;
    let nonce = crate::hooks::random_token().unwrap_or_else(|| "status".into());
    let directory = PathBuf::from(format!(
        "/tmp/zerocode-computer-permissions-{}-{}",
        std::process::id(),
        &nonce[..nonce.len().min(10)]
    ));
    if directory.exists() {
        std::fs::remove_dir_all(&directory).map_err(io_error("remove stale permission probe"))?;
    }
    std::fs::create_dir(&directory).map_err(io_error("create permission probe"))?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .map_err(io_error("protect permission probe"))?;
    let status = directory.join("status.json");
    let mut command = crate::proc::quiet_command("/usr/bin/open");
    command.args(permission_probe_args(&helper, &status, requested));
    let launched = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(io_error("launch permission probe"))?;
    if !launched.success() {
        cleanup(&directory);
        return Err(ComputerUseError::new(
            error_code::ACCESSIBILITY_ERROR,
            "could not launch the Computer Use permission probe",
        ));
    }
    let started = std::time::Instant::now();
    let text = loop {
        if let Ok(text) = std::fs::read_to_string(&status) {
            break text;
        }
        if started.elapsed() >= PERMISSION_PROBE_TIMEOUT {
            cleanup(&directory);
            return Err(ComputerUseError::new(
                error_code::ACTION_TIMEOUT,
                "timed out checking Computer Use permissions",
            ));
        }
        std::thread::sleep(PERMISSION_POLL_INTERVAL);
    };
    cleanup(&directory);
    let raw: Value = serde_json::from_str(&text).map_err(|error| {
        ComputerUseError::new(error_code::ACCESSIBILITY_ERROR, error.to_string())
    })?;
    let status_of = |id: ComputerPermissionId| ComputerPermissionState {
        id,
        status: if raw.get(id.as_str()).and_then(Value::as_str) == Some("granted") {
            ComputerPermissionStatus::Granted
        } else {
            ComputerPermissionStatus::NotGranted
        },
    };
    let permissions = vec![
        status_of(ComputerPermissionId::Accessibility),
        status_of(ComputerPermissionId::Screenshots),
    ];
    Ok((
        ComputerPermissionReport {
            identity: helper_signing_identity(),
            platform: "darwin".into(),
            helper_app_path: Some(helper.to_string_lossy().into_owned()),
            helper_unavailable_reason: None,
            tcc_rows: tcc_rows(&helper, Some(&permissions)),
            permissions,
            judged_rows: judged_rows(&helper),
        },
        raw.get("requested_os").and_then(Value::as_bool) == Some(true),
    ))
}

#[derive(Debug, serde::Deserialize)]
struct PermissionTarget {
    id: ComputerPermissionId,
    service: String,
    settings_url: String,
    list_name: String,
    subject: PermissionSubject,
    /// The TCC database this service's rows stand in.
    database: tcc::TccDatabase,
}

/// Which process a permission list judges. Accessibility looks at the caller
/// (the helper); Screen Recording looks at the responsible process (the app
/// that opened the helper), so its row wears the app's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum PermissionSubject {
    Helper,
    App,
}

impl PermissionSubject {
    /// This app's two bundles — the only ones whose rows are read or reset.
    const BOTH: [Self; 2] = [Self::Helper, Self::App];

    const fn bundle_id(self) -> &'static str {
        match self {
            Self::Helper => HELPER_BUNDLE_ID,
            Self::App => APP_BUNDLE_ID,
        }
    }

    const fn other(self) -> Self {
        match self {
            Self::Helper => Self::App,
            Self::App => Self::Helper,
        }
    }
}

/// The System Settings row `subject`'s bundle has for permission `id`.
fn subject_row(
    id: ComputerPermissionId,
    subject: PermissionSubject,
    helper: &Path,
) -> ComputerPermissionRow {
    let (name, path) = match subject {
        PermissionSubject::Helper => (HELPER_NAME, helper.to_path_buf()),
        PermissionSubject::App => (APP_NAME, app_bundle_path(helper)),
    };
    ComputerPermissionRow {
        id,
        bundle_id: subject.bundle_id().into(),
        name: name.into(),
        path: path.to_string_lossy().into_owned(),
    }
}

/// The System Settings row `target` is judged on.
fn judged_row(target: &PermissionTarget, helper: &Path) -> ComputerPermissionRow {
    subject_row(target.id, target.subject, helper)
}

/// Both bundles' rows for `target` — the judged one first — and whether each
/// has a bundle signature to be held against. A helper standing alone (a dev
/// build) has no app bundle around it, so the app's row has none.
fn tcc_wanted(
    target: &PermissionTarget,
    helper: &Path,
) -> [(bool, ComputerPermissionRow, bool); 2] {
    let standing_alone = app_bundle_path(helper) == helper;
    [target.subject, target.subject.other()].map(|subject| {
        (
            subject == target.subject,
            subject_row(target.id, subject, helper),
            subject == PermissionSubject::Helper || !standing_alone,
        )
    })
}

/// [`tcc_wanted`]'s rows, each with what the TCC database records for it,
/// held against its bundle as signed now.
fn tcc_reads(
    target: &PermissionTarget,
    helper: &Path,
) -> Vec<(bool, ComputerPermissionRow, tcc::RowRead)> {
    let rows = tcc_wanted(target, helper);
    let wanted = rows
        .iter()
        .map(|(_, row, signed)| tcc::Wanted {
            service: &target.service,
            bundle_id: &row.bundle_id,
            bundle: signed.then(|| Path::new(&row.path)),
        })
        .collect::<Vec<_>>();
    let reads = target.database.path().map_or_else(
        || vec![tcc::RowRead::Unreadable(ComputerPermissionUnreadable::Database); wanted.len()],
        |database| tcc::read_rows(&database, &wanted),
    );
    rows.into_iter()
        .zip(reads)
        .map(|((judged, row, _), read)| (judged, row, read))
        .collect()
}

/// `target`'s rows as read, each judged: macOS's answer for the permission
/// (`live`, the helper probe's) speaks for the row it is judged on and for no
/// other — the helper granted Accessibility says nothing of the app's own
/// Accessibility row, which is the row 2026-09-22 found stale.
fn judge_rows(
    target: &PermissionTarget,
    reads: Vec<(bool, ComputerPermissionRow, tcc::RowRead)>,
    live: Option<&[ComputerPermissionState]>,
) -> Vec<ComputerPermissionTccRow> {
    let answer = live
        .and_then(|states| states.iter().find(|state| state.id == target.id))
        .map(|state| state.status == ComputerPermissionStatus::Granted);
    reads
        .into_iter()
        .map(|(judged, row, read)| {
            let (grant, unreadable, pin) = tcc::verdict(answer.filter(|_| judged), read);
            ComputerPermissionTccRow::new(row, judged, grant, unreadable, pin)
        })
        .collect()
}

/// Every permission's two rows, as the settings page and `permissions` show
/// them. `live` is the helper probe's answer — `None` when the probe did not
/// answer.
fn tcc_rows(
    helper: &Path,
    live: Option<&[ComputerPermissionState]>,
) -> Vec<ComputerPermissionTccRow> {
    permission_targets()
        .unwrap_or_default()
        .iter()
        .flat_map(|target| judge_rows(target, tcc_reads(target, helper), live))
        .collect()
}

/// Whether a row read as `read` offers `action`: the table's buttons for the
/// grant the database alone gives it — the door the person's click passes.
fn row_offers(read: tcc::RowRead, action: ComputerPermissionRowAction) -> bool {
    tcc::verdict(None, read).0.actions().contains(&action)
}

/// The app bundle the helper ships inside — the nearest `.app` above it. A
/// helper standing alone (a dev build) is its own responsible process.
fn app_bundle_path(helper: &Path) -> PathBuf {
    helper
        .ancestors()
        .skip(1)
        .find(|ancestor| ancestor.extension().is_some_and(|ext| ext == "app"))
        .map_or_else(|| helper.to_path_buf(), Path::to_path_buf)
}

fn judged_rows(helper: &Path) -> Vec<ComputerPermissionRow> {
    [
        ComputerPermissionId::Accessibility,
        ComputerPermissionId::Screenshots,
    ]
    .into_iter()
    .filter_map(|id| permission_target(id).ok())
    .map(|target| judged_row(&target, helper))
    .collect()
}

fn helper_or_name() -> PathBuf {
    helper_app_path().unwrap_or_else(|| PathBuf::from(HELPER_APP_NAME))
}

fn permission_targets() -> Result<Vec<PermissionTarget>, ComputerUseError> {
    serde_json::from_str(include_str!(
        "../../native/computer-use-macos/permissions.json"
    ))
    .map_err(|error| ComputerUseError::invalid_argument(error.to_string()))
}

fn permission_target(id: ComputerPermissionId) -> Result<PermissionTarget, ComputerUseError> {
    permission_targets()?
        .into_iter()
        .find(|target| target.id == id)
        .ok_or_else(|| ComputerUseError::invalid_argument("missing permission target"))
}

fn permission_probe_args(
    helper: &Path,
    status: &Path,
    requested: Option<ComputerPermissionId>,
) -> Vec<String> {
    let mode = if requested.is_some() {
        "--permission-request-file"
    } else {
        "--permission-status-file"
    };
    let mut args = vec![
        "-n".into(),
        helper.to_string_lossy().into_owned(),
        "--args".into(),
        mode.into(),
        status.to_string_lossy().into_owned(),
    ];
    if let Some(id) = requested {
        args.push(id.as_str().into());
    }
    args
}

/// `tccutil reset <service> <bundle id>` — always three words, the last one
/// of this app's own two bundle ids. `tccutil` reads a missing bundle id as
/// every app's row for the service, so anything else is refused here, before
/// a process starts (t-6058).
fn reset_args<'a>(
    target: &'a PermissionTarget,
    bundle_id: &'a str,
) -> Result<[&'a str; 3], ComputerUseError> {
    if !PermissionSubject::BOTH
        .iter()
        .any(|subject| subject.bundle_id() == bundle_id)
    {
        return Err(ComputerUseError::invalid_argument(format!(
            "tccutil reset names only this app's own rows, not {bundle_id:?}"
        )));
    }
    Ok(["reset", &target.service, bundle_id])
}

/// Launching the helper's own permission window: the small "Enable ZeroCode
/// Computer Use" panel with the draggable tile that the person drops into the
/// System Settings list. `open -n` so it is its own responsible process (the
/// grant binds to the helper's signature, not the window's).
fn permission_assistant_args(helper: &Path, id: ComputerPermissionId) -> Vec<String> {
    vec![
        "-n".into(),
        helper.to_string_lossy().into_owned(),
        "--args".into(),
        "--permission".into(),
        id.as_str().into(),
    ]
}

/// Bring up the drag-and-drop assistant for `id`, fire-and-forget — it is a
/// GUI window the person interacts with, not a probe we wait on.
fn launch_permission_assistant(helper: &Path, id: ComputerPermissionId) {
    let _ = crate::proc::quiet_command("/usr/bin/open")
        .args(permission_assistant_args(helper, id))
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// What a `permissions` call does: only a named permission is requested of
/// the OS (prompt plus the exact System Settings list); a plain status call
/// requests nothing and opens nothing — it merely names the first missing
/// row so the person knows where to go. Polling status must never pop
/// System Settings.
#[derive(Debug, PartialEq, Eq)]
struct PermissionPlan {
    request: Option<ComputerPermissionId>,
    advise: Option<ComputerPermissionId>,
}

fn permission_plan(
    asked: Option<ComputerPermissionId>,
    states: &[ComputerPermissionState],
) -> PermissionPlan {
    PermissionPlan {
        request: asked,
        advise: asked.or_else(|| {
            states
                .iter()
                .find(|state| state.status == ComputerPermissionStatus::NotGranted)
                .map(|state| state.id)
        }),
    }
}

fn permission_next_step(target: &PermissionTarget, row: &ComputerPermissionRow) -> String {
    format!(
        "In System Settings > Privacy & Security > {}, turn on {}. If the row is missing, use + to add {}. Then check permissions again to restart the helper. If it is already on but still not-granted, run zerocode-computer permissions --id {} --reset, enable the new row, and check again.",
        target.list_name,
        row.name,
        row.path,
        target.id.as_str()
    )
}

pub(super) fn open_permission(
    permission_id: Option<ComputerPermissionId>,
) -> Result<ComputerPermissionSetup, ComputerUseError> {
    let report = if permission_id.is_some() {
        // A named permission is asked of the OS and the person sent to its
        // list: the session ends here, and the last reading is forgotten.
        super::shutdown();
        forget_rows_read();
        permission_status_macos()?
    } else {
        // A plain status ask is a reading like the window's own — an agent
        // makes one as the first call of every run — and ends the session
        // only when a row moved (t-109717).
        let report = permission_status_macos()?;
        settle_session_for(&report.permissions);
        report
    };
    let plan = permission_plan(permission_id, &report.permissions);
    let mut setup = ComputerPermissionSetup {
        identity: report.identity,
        platform: report.platform,
        helper_app_path: report.helper_app_path,
        judged_rows: report.judged_rows,
        tcc_rows: report.tcc_rows,
        permission_id: plan.advise,
        requested_os: false,
        opened_settings: false,
        launched_helper: true,
        permissions: Some(report.permissions),
        next_step: None,
    };
    if let (None, Some(id)) = (plan.request, plan.advise) {
        let target = permission_target(id)?;
        setup.next_step = Some(permission_next_step(
            &target,
            &judged_row(&target, &helper_or_name()),
        ));
    }
    if let Some(id) = plan.request {
        let target = permission_target(id)?;
        // Show the person the helper's own "drag this into the list" window,
        // the way ChatGPT's and Orca's helpers do — not just the OS pane.
        if let Some(helper) = helper_app_path() {
            launch_permission_assistant(&helper, id);
        }
        let (report, requested_os) = permission_probe(Some(id))?;
        if !requested_os {
            return Err(ComputerUseError::new(
                error_code::PROVIDER_INCOMPATIBLE,
                "Computer Use helper did not acknowledge the OS permission request",
            ));
        }
        setup.requested_os = requested_os;
        setup.permissions = Some(report.permissions);
        setup.tcc_rows = report.tcc_rows;
        open_settings_pane(&target)?;
        setup.opened_settings = true;
        setup.next_step = Some(permission_next_step(
            &target,
            &judged_row(&target, &helper_or_name()),
        ));
    }
    Ok(setup)
}

/// The permission's pane in System Settings.
fn open_settings_pane(target: &PermissionTarget) -> Result<(), ComputerUseError> {
    let opened = crate::proc::quiet_command("/usr/bin/open")
        .arg(&target.settings_url)
        .status()
        .map_err(io_error("open permission settings"))?;
    if !opened.success() {
        return Err(ComputerUseError::new(
            error_code::ACCESSIBILITY_ERROR,
            "could not open the permission settings page",
        ));
    }
    Ok(())
}

/// `tccutil reset` for one of this app's rows — the one road to it.
fn reset_row(
    target: &PermissionTarget,
    row: &ComputerPermissionRow,
) -> Result<(), ComputerUseError> {
    super::shutdown();
    let output = crate::proc::quiet_command("/usr/bin/tccutil")
        .args(reset_args(target, &row.bundle_id)?)
        .output()
        .map_err(io_error("reset Computer Use permission"))?;
    // The reset ran: the row is no longer what the last reading said.
    forget_rows_read();
    if !output.status.success() {
        let why = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(ComputerUseError::new(
            error_code::ACCESSIBILITY_ERROR,
            format!("could not reset {} for {}: {why}", target.service, row.name),
        ));
    }
    Ok(())
}

/// Reset the row `id` is judged on; returns the bundle id that row named.
pub(super) fn reset_permission(id: ComputerPermissionId) -> Result<String, ComputerUseError> {
    let target = permission_target(id)?;
    let row = judged_row(&target, &helper_or_name());
    reset_row(&target, &row)?;
    Ok(row.bundle_id)
}

/// A TCC row's button, pressed (t-6058): `action` on `bundle_id`'s row of
/// `id`'s service — only while that row, as the database records it now,
/// reads as a grant the table gives that button, so nothing but a stale row
/// is ever reset from here. Answers the report read after it: the row says
/// what the button did.
pub(super) fn tcc_row_action(
    id: ComputerPermissionId,
    bundle_id: &str,
    action: ComputerPermissionRowAction,
) -> Result<ComputerPermissionReport, ComputerUseError> {
    let target = permission_target(id)?;
    let Some((_, row, read)) = tcc_reads(&target, &helper_or_name())
        .into_iter()
        .find(|(_, row, _)| row.bundle_id == bundle_id)
    else {
        return Err(ComputerUseError::invalid_argument(format!(
            "{bundle_id:?} names none of this app's rows"
        )));
    };
    if !row_offers(read, action) {
        return Err(ComputerUseError::invalid_argument(format!(
            "{}'s {} row offers no {action:?} as it reads now",
            row.name, target.service
        )));
    }
    match action {
        ComputerPermissionRowAction::Reset => reset_row(&target, &row)?,
        ComputerPermissionRowAction::OpenSettings => open_settings_pane(&target)?,
    }
    Ok(permission_status())
}

pub(super) fn reset_permissions() -> Result<ComputerPermissionReset, ComputerUseError> {
    let mut bundle_ids = Vec::new();
    for state in missing_permissions() {
        bundle_ids.push(reset_permission(state.id)?);
    }
    Ok(ComputerPermissionReset {
        report: permission_status(),
        bundle_ids,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_recording_is_judged_on_the_apps_row_and_accessibility_on_the_helpers() {
        // tccd: Accessibility judges the calling process, ScreenCapture the
        // responsible process — the window that opened the helper. Telling
        // the person to turn on the helper's row for Screen Recording (and
        // resetting the helper's row) leaves them granted-yet-denied.
        let helper =
            Path::new("/Applications/ZeroCode.app/Contents/Resources/ZeroCode Computer Use.app");
        let screenshots = permission_target(ComputerPermissionId::Screenshots).unwrap();
        let row = judged_row(&screenshots, helper);
        assert_eq!(row.bundle_id, APP_BUNDLE_ID);
        assert_eq!(row.name, APP_NAME);
        assert_eq!(row.path, "/Applications/ZeroCode.app");
        assert_eq!(
            reset_args(&screenshots, &row.bundle_id).unwrap(),
            ["reset", "ScreenCapture", APP_BUNDLE_ID]
        );
        let step = permission_next_step(&screenshots, &row);
        assert!(step.contains("turn on ZeroCode."), "{step}");
        assert!(!step.contains(HELPER_NAME), "{step}");
        let accessibility = permission_target(ComputerPermissionId::Accessibility).unwrap();
        let row = judged_row(&accessibility, helper);
        assert_eq!(
            (row.bundle_id.as_str(), row.name.as_str()),
            (HELPER_BUNDLE_ID, HELPER_NAME)
        );
        assert_eq!(row.path, helper.to_string_lossy());
    }

    #[test]
    fn stale_or_missing_helper_budget_is_not_reported_as_unlimited() {
        let current = json!({"supports":{"desktop":{"guard":{
            "budget": zerocode_core::computer_use::guard_table()
        }}}});
        assert!(validate_guard_budget(&current).is_ok());
        let old = json!({"supports":{"desktop":{"guard":{"budget":{
            "perSecond":10,"burst":20,"perSession":5000
        }}}}});
        for handshake in [
            old,
            json!({}),
            json!({"supports":{"desktop":{"guard":{"budget":null}}}}),
        ] {
            assert_eq!(
                validate_guard_budget(&handshake).unwrap_err().code,
                error_code::PROVIDER_INCOMPATIBLE
            );
        }
    }

    /// The helper counts by the core's table, and it reaches the helper whole
    /// through `open --env`: one `NAME=json` word, split at its first `=`.
    #[test]
    fn the_helper_is_launched_with_the_cores_guard_table() {
        let word = guard_table_env();
        let (name, table) = word.split_once('=').expect("NAME=value");
        assert_eq!(name, zerocode_core::computer_use::COMPUTER_GUARD_TABLE_ENV);
        assert_eq!(
            serde_json::from_str::<Value>(table).expect("json"),
            Value::Object(zerocode_core::computer_use::guard_table())
        );
        let source = include_str!("macos.rs");
        let launch = &source[source.find("fn launch()").expect("launch")..];
        let launch = &launch[..launch.find("let launched").expect("the launch")];
        let env = launch
            .find(".arg(guard_table_env())")
            .expect("the table rides the launch");
        assert!(
            env < launch.find(".arg(\"--args\")").expect("--args"),
            "`open` reads --env only before --args"
        );
    }

    /// The window's road to the helper is the launch and then the connection
    /// with its handshake, and nothing else (t-66753 C): the stand-in tests
    /// drive `Session::connect` alone, so `start` must be exactly the two,
    /// in that order, for what they prove to be the product's own road.
    #[test]
    fn the_session_starts_by_launching_the_helper_then_connecting_to_it() {
        let source = include_str!("macos.rs");
        let start = &source[source
            .find("    fn start() -> Result<Self, ComputerUseError> {")
            .expect("start")..];
        let body = &start[..start.find("\n    }\n").expect("start's end")];
        let said: Vec<&str> = body.lines().skip(1).map(str::trim).collect();
        assert_eq!(
            said,
            ["Self::connect(launch()?)"],
            "start is the launch, then the connection"
        );
    }

    /// The window's waiting request is a road the helper answers outside
    /// its provider lock (t-66753 B): beside an action in flight, as the
    /// stop and the status are.
    #[test]
    fn the_waiting_request_is_one_of_the_helpers_unlocked_roads() {
        let helper = include_str!(
            "../../native/computer-use-macos/Sources/ZeroCodeComputerUseMacOS/main.swift"
        );
        let roads = helper
            .split("func unlockedRoad(")
            .nth(1)
            .expect("the helper's unlocked roads");
        let roads = &roads[..roads.find("\n}\n").expect("their end")];
        assert!(
            roads.contains(&format!("case \"{STOP_WATCH_METHOD}\":")),
            "the helper answers the window's waiting request outside its provider lock"
        );
    }

    /// `tccutil reset <service>` without a bundle id resets every app's row
    /// for the service — the person's grants to every other app. So a reset
    /// is three words or nothing: a bundle that is not one of this app's own
    /// two is refused before anything runs (t-6058).
    #[test]
    fn a_reset_names_one_of_this_apps_bundles_or_runs_nothing() {
        let target = permission_target(ComputerPermissionId::Accessibility).unwrap();
        for bundle in [HELPER_BUNDLE_ID, APP_BUNDLE_ID] {
            assert_eq!(
                reset_args(&target, bundle).unwrap(),
                ["reset", "Accessibility", bundle]
            );
        }
        for stranger in [
            "",
            " ",
            "com.apple.Terminal",
            "dev.zerocode.app ",
            "dev.zerocode",
        ] {
            assert!(
                reset_args(&target, stranger).is_err(),
                "{stranger:?} reached tccutil"
            );
        }
    }

    /// Both bundles' rows are read for each permission, the judged one first;
    /// a helper standing alone has no app bundle to hold the app's row
    /// against.
    #[test]
    fn both_bundles_rows_are_read_for_every_permission_the_judged_one_first() {
        let helper =
            Path::new("/Applications/ZeroCode.app/Contents/Resources/ZeroCode Computer Use.app");
        let accessibility = permission_target(ComputerPermissionId::Accessibility).unwrap();
        let screenshots = permission_target(ComputerPermissionId::Screenshots).unwrap();
        let bundles = |target: &PermissionTarget, helper: &Path| {
            tcc_wanted(target, helper).map(|(judged, row, signed)| (judged, row.bundle_id, signed))
        };
        assert_eq!(
            bundles(&accessibility, helper),
            [
                (true, HELPER_BUNDLE_ID.to_string(), true),
                (false, APP_BUNDLE_ID.to_string(), true),
            ]
        );
        assert_eq!(
            bundles(&screenshots, helper),
            [
                (true, APP_BUNDLE_ID.to_string(), true),
                (false, HELPER_BUNDLE_ID.to_string(), true),
            ]
        );
        let dev =
            Path::new("/repo/native/computer-use-macos/.build/release/ZeroCode Computer Use.app");
        assert_eq!(
            bundles(&screenshots, dev),
            [
                (true, APP_BUNDLE_ID.to_string(), false),
                (false, HELPER_BUNDLE_ID.to_string(), true),
            ]
        );
        for target in [&accessibility, &screenshots] {
            assert_eq!(
                target.database,
                tcc::TccDatabase::System,
                "measured 2026-09-26"
            );
        }
    }

    /// 2026-09-22 as the page reads it: the helper's Accessibility is granted
    /// — macOS said so — and the app's own Accessibility row is pinned to the
    /// 09-08 build. macOS's answer speaks for the judged row only, so the
    /// app's row still reads stale and offers its reset and its pane.
    #[test]
    fn macos_answer_speaks_for_the_judged_row_and_the_pinned_row_still_reads_stale() {
        use zerocode_core::computer_use::{ComputerPermissionGrant, ComputerPermissionPin};
        let helper =
            Path::new("/Applications/ZeroCode.app/Contents/Resources/ZeroCode Computer Use.app");
        let target = permission_target(ComputerPermissionId::Accessibility).unwrap();
        let [(judged, helper_row, _), (other, app_row, _)] = tcc_wanted(&target, helper);
        let reads = vec![
            (
                judged,
                helper_row,
                tcc::RowRead::Allows {
                    pin: Some(ComputerPermissionPin::Signature),
                    satisfied: true,
                },
            ),
            (
                other,
                app_row,
                tcc::RowRead::Allows {
                    pin: Some(ComputerPermissionPin::Cdhash),
                    satisfied: false,
                },
            ),
        ];
        let live = [ComputerPermissionState {
            id: ComputerPermissionId::Accessibility,
            status: ComputerPermissionStatus::Granted,
        }];
        let rows = judge_rows(&target, reads, Some(&live));
        let read = rows
            .iter()
            .map(|row| {
                (
                    row.row.bundle_id.as_str(),
                    row.judged,
                    row.grant,
                    row.pin,
                    row.actions.clone(),
                )
            })
            .collect::<Vec<_>>();
        assert_eq!(
            read,
            [
                (
                    HELPER_BUNDLE_ID,
                    true,
                    ComputerPermissionGrant::Granted,
                    Some(ComputerPermissionPin::Signature),
                    vec![],
                ),
                (
                    APP_BUNDLE_ID,
                    false,
                    ComputerPermissionGrant::Stale,
                    Some(ComputerPermissionPin::Cdhash),
                    vec![
                        ComputerPermissionRowAction::Reset,
                        ComputerPermissionRowAction::OpenSettings,
                    ],
                ),
            ]
        );
    }

    /// The row door lets through only what the table gives the row's grant as
    /// the database records it: a stale row is reset and opened, and nothing
    /// else is reset — not a working grant, not an absent row, not a row that
    /// could not be read.
    #[test]
    fn only_a_stale_row_is_reset_through_its_door() {
        use zerocode_core::computer_use::{ComputerPermissionPin, ComputerPermissionUnreadable};
        let stale = tcc::RowRead::Allows {
            pin: Some(ComputerPermissionPin::Cdhash),
            satisfied: false,
        };
        assert!(row_offers(stale, ComputerPermissionRowAction::Reset));
        assert!(row_offers(stale, ComputerPermissionRowAction::OpenSettings));
        for read in [
            tcc::RowRead::Allows {
                pin: Some(ComputerPermissionPin::Signature),
                satisfied: true,
            },
            tcc::RowRead::Refused,
            tcc::RowRead::Unreadable(ComputerPermissionUnreadable::NoFullDiskAccess),
        ] {
            for action in [
                ComputerPermissionRowAction::Reset,
                ComputerPermissionRowAction::OpenSettings,
            ] {
                assert!(!row_offers(read, action), "{read:?} offered {action:?}");
            }
        }
    }

    /// This machine's rows as the settings page reads them, and what the
    /// reading costs — the before/after measure of t-6058. Reads the system
    /// TCC database (this app's rows only) and launches the installed
    /// helper's status probe, which requests nothing.
    #[test]
    #[ignore = "reads this machine's TCC database and launches the installed helper's status probe; run with ZEROCODE_COMPUTER_MACOS_HELPER_APP_PATH naming the installed helper"]
    fn this_machines_rows_as_the_settings_page_reads_them() {
        let report = permission_status();
        eprintln!("{}", serde_json::to_string_pretty(&report).unwrap());
        let helper = helper_app_path().expect("the installed helper");
        let mut spent = (0..21)
            .map(|_| {
                let started = std::time::Instant::now();
                let rows = tcc_rows(&helper, Some(&report.permissions));
                assert_eq!(rows.len(), 4);
                started.elapsed()
            })
            .collect::<Vec<_>>();
        spent.sort();
        eprintln!(
            "tcc_rows over 21 reads: p50 {:?}, max {:?}",
            spent[10], spent[20]
        );
        assert_eq!(
            report.tcc_rows.len(),
            4,
            "both bundles' rows for both permissions"
        );
    }

    #[test]
    fn a_helper_standing_alone_is_its_own_responsible_process() {
        let dev =
            Path::new("/repo/native/computer-use-macos/.build/release/ZeroCode Computer Use.app");
        assert_eq!(app_bundle_path(dev), dev);
    }

    #[test]
    #[ignore = "interactive macOS TCC request; run only with the person ready to grant permissions, with ZEROCODE_COMPUTER_MACOS_HELPER_APP_PATH naming the built helper"]
    fn computer_use_interactive_permission_request() {
        let id = std::env::var("ZEROCODE_TEST_PERMISSION")
            .expect("name the permission to request")
            .parse()
            .unwrap();
        let reset = std::env::var("ZEROCODE_TEST_PERMISSION_RESET").as_deref() == Ok("1");
        let setup = super::super::setup_permission(Some(id), reset).unwrap();
        eprintln!("{}", serde_json::to_string(&setup).unwrap());
        assert!(setup.requested_os && setup.opened_settings);
    }

    #[test]
    #[ignore = "interactive macOS TCC verification after the person grants both permissions, with ZEROCODE_COMPUTER_MACOS_HELPER_APP_PATH naming the built helper"]
    fn computer_use_interactive_grant_survives_session_restart() {
        let report = permission_status();
        eprintln!("{}", serde_json::to_string(&report).unwrap());
        assert!(
            report
                .permissions
                .iter()
                .all(|row| row.status == ComputerPermissionStatus::Granted)
        );
        super::super::call("handshake", json!({})).unwrap();
        assert!(super::super::session_stands());
        let refreshed = permission_status();
        assert!(
            super::super::session_stands(),
            "a reading that moved no row leaves the session standing"
        );
        super::super::shutdown();
        assert!(!super::super::session_stands());
        assert!(
            refreshed
                .permissions
                .iter()
                .all(|row| row.status == ComputerPermissionStatus::Granted)
        );
        super::super::call("screenshotDesktop", json!({})).unwrap();
        super::super::shutdown();
    }

    #[test]
    fn computer_use_permission_targets_choose_exact_settings_and_reset_service() {
        for (id, service, pane) in [
            (
                ComputerPermissionId::Accessibility,
                "Accessibility",
                "Privacy_Accessibility",
            ),
            (
                ComputerPermissionId::Screenshots,
                "ScreenCapture",
                "Privacy_ScreenCapture",
            ),
        ] {
            let target = permission_target(id).unwrap();
            assert_eq!(
                target.settings_url,
                format!("x-apple.systempreferences:com.apple.preference.security?{pane}")
            );
            let row = judged_row(&target, Path::new("/test/helper.app"));
            assert_eq!(
                reset_args(&target, &row.bundle_id).unwrap(),
                ["reset", service, row.bundle_id.as_str()]
            );
            assert!(permission_next_step(&target, &row).contains(&format!(
                "turn on {}. If the row is missing, use + to add {}",
                row.name, row.path
            )));
            assert_eq!(
                permission_assistant_args(Path::new("/test/helper.app"), id),
                vec![
                    "-n".to_string(),
                    "/test/helper.app".to_string(),
                    "--args".to_string(),
                    "--permission".to_string(),
                    id.as_str().to_string(),
                ],
                "the request path opens the helper's drag-and-drop window, not only System Settings"
            );
        }
    }

    #[test]
    fn computer_use_permission_probe_argv_preserves_paths_and_requests_only_when_explicit() {
        let helper = Path::new("/test/Helper App.app");
        let status = Path::new("/test/state.json");
        assert_eq!(
            permission_probe_args(helper, status, Some(ComputerPermissionId::Screenshots)),
            [
                "-n",
                "/test/Helper App.app",
                "--args",
                "--permission-request-file",
                "/test/state.json",
                "screenshots"
            ]
        );
        assert_eq!(
            permission_probe_args(helper, status, None).last().unwrap(),
            "/test/state.json"
        );
        // A status call requests nothing of the OS: polling it must never pop
        // System Settings (it did, every three seconds, on 2026-09-08).
        assert_eq!(
            permission_plan(None, &missing_permissions()),
            PermissionPlan {
                request: None,
                advise: Some(ComputerPermissionId::Accessibility)
            }
        );
        assert_eq!(
            permission_plan(
                Some(ComputerPermissionId::Screenshots),
                &missing_permissions()
            ),
            PermissionPlan {
                request: Some(ComputerPermissionId::Screenshots),
                advise: Some(ComputerPermissionId::Screenshots)
            }
        );
        assert_eq!(
            permission_plan(None, &[]),
            PermissionPlan {
                request: None,
                advise: None
            }
        );
    }

    #[test]
    fn computer_use_identity_reports_the_actual_signature() {
        use zerocode_core::computer_use::ComputerSigningIdentity::{Adhoc, Local};
        assert_eq!(signing_identity("Signature=adhoc"), Some(Adhoc));
        assert_eq!(
            signing_identity("designated => identifier app and certificate leaf = H\"ABC\""),
            Some(Local)
        );
        assert_eq!(signing_identity("unsigned"), None);
    }

    #[test]
    fn helper_paths_keep_one_stable_tcc_identity() {
        assert_eq!(HELPER_APP_NAME, "ZeroCode Computer Use.app");
        assert_eq!(HELPER_BUNDLE_ID, "dev.zerocode.app.computer-use");
        assert_eq!(HELPER_EXECUTABLE, "zerocode-computer-use-macos");
    }

    #[test]
    #[ignore = "live helper + TCC integration; on a machine with both grants the helper refuses the test process as an unauthorized peer (t-3620) — run with --ignored once that is settled, with ZEROCODE_COMPUTER_MACOS_HELPER_APP_PATH naming the built helper"]
    fn the_built_helper_and_rust_owner_complete_the_authenticated_handshake() {
        super::super::shutdown();
        const TEST_NAME: &str = "authenticated Computer Use handshake";

        // This is an integration test against the locally built helper, whose
        // TCC grants belong to its signed bundle identity rather than to this
        // checkout. A fresh target directory can therefore build the Rust
        // test without having either the helper app or its Accessibility and
        // Screen Recording approvals. Probe all of those prerequisites before
        // opening the authenticated socket: `helper_executable_path()` checks
        // the app's executable, and `permission_status_macos()` asks the
        // helper's status probe for both TCC states. An unprepared environment
        // gets a useful reason and returns, while an approved one runs every
        // check below.
        if helper_executable_path().is_none() {
            eprintln!("skipping {TEST_NAME}: the built helper executable is unavailable");
            return;
        }
        let report = match permission_status_macos() {
            Ok(report) => report,
            Err(error) => {
                eprintln!("skipping {TEST_NAME}: could not check helper permissions: {error}");
                return;
            }
        };
        let missing = report
            .permissions
            .iter()
            .filter(|permission| permission.status != ComputerPermissionStatus::Granted)
            .map(|permission| permission.id.as_str())
            .collect::<Vec<_>>();
        if !missing.is_empty() {
            eprintln!(
                "skipping {TEST_NAME}: helper permissions not granted: {}",
                missing.join(", ")
            );
            return;
        }

        let capabilities =
            super::super::call("handshake", serde_json::json!({})).expect("helper handshake");
        assert_eq!(
            capabilities
                .get("protocolVersion")
                .and_then(serde_json::Value::as_u64),
            Some(COMPUTER_USE_PROTOCOL_VERSION)
        );
        assert_eq!(
            capabilities
                .get("provider")
                .and_then(serde_json::Value::as_str),
            Some("zerocode-computer-use-macos")
        );
        super::super::shutdown();
    }
}
