//! Running `zo` on behalf of the IDE.
//!
//! Three rules meet the operating system here:
//!
//! 1. **The session outlives the window.** `zo serve` owns the session pool, so
//!    the IDE starts it *detached* — never as a child of our pty, never in our
//!    process group. Closing the IDE must detach a lane, not kill the work.
//! 2. **`zo` is optional.** The application discovers the binary before it
//!    constructs a supervisor; without `zo`, lane surfaces fold away while the
//!    editor, file tree, diff and terminal keep working.
//! 3. **Loopback and a shared secret, always.** A session server can run any
//!    command in the user's project. Before this crate hands anything —
//!    including the token — to a listening port, it checks the unauthenticated
//!    session response shape, then verifies that the listener accepts our
//!    secret. The product assumes a single-user workstation where every local
//!    loopback listener is trusted.
//!
//! A lane itself is `zo attach <session>` hosted in a pty, so the user sees the
//! real `zo` TUI rather than a reimplementation of it.

pub mod addr;
pub mod app_paths;
mod channel_file;
pub mod host_pty;
pub mod pty_transport;
pub mod registry;
pub mod token;

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use zerocode_core::host::{PtyCwd, PtySpec};
use zerocode_pty::{ZO_EVENTS_ADDR_FILE_ENV, ZoBinary};

pub use addr::{default_bind_for, project_port, project_root};
pub use app_paths::{
    APP_IDENTIFIER, AppPaths, LegacyAuthorityLease, PATH_ACTIVATION_VERSION,
    PATH_ACTIVATION_WITNESS, PATH_MIGRATION_LOCK, PATH_MIGRATION_MANIFEST_ID,
    PATH_MIGRATION_RECEIPT, PATH_MIGRATION_VERSION, PREFERENCES_FILE, PathAuthority, PathClass,
    PathMigrationLock, PathMigrationReceipt, artifact_manifest_id, ensure_private_app_dir,
    legacy_state_root, read_complete_path_migration_receipt,
};
pub use channel_file::{
    PaneChannelIdentity, discover_pane_channel, pane_channel_file, read_pane_channel,
};
pub use host_pty::{LocalPty, PtySpawner};
pub use pty_transport::{PtyHandle, PtyTransport, PtyTransportError};
pub use registry::{LaneEvent, LaneRegistry, QUIET_PUMPS_BEFORE_IDLE, RegistryError, Removed};
pub use token::{
    PlatformTokenError, SERVE_TOKEN_PREFIX, is_canonical_serve_token_name,
    is_owned_staged_serve_token_name, project_token, project_token_for_platform,
};

/// Environment variable the server and its clients use for the shared secret.
pub const TOKEN_ENV: &str = "ZO_SERVE_TOKEN";

/// How long to wait for a just-started server to answer the protocol.
pub const DEFAULT_SERVE_READY_TIMEOUT: Duration = Duration::from_secs(10);

/// What an IDE pane asks for: loopback, with the port chosen atomically by the
/// kernel and reported through [`ZO_EVENTS_ADDR_FILE_ENV`].
pub const PANE_CHANNEL_BIND: &str = "127.0.0.1:0";

/// How long a freshly spawned pane gets to publish its channel address.
pub const DEFAULT_PANE_CHANNEL_READY_TIMEOUT: Duration = Duration::from_secs(20);

/// Budget for one probe: connect, ask, read a line. Short, because this runs on
/// the path that decides whether to show the lane panel at all.
const PROBE_TIMEOUT: Duration = Duration::from_millis(750);

/// Cap on the probe reply we will buffer. A well-behaved server answers in a
/// few hundred bytes; anything larger is not something to read into memory.
const MAX_PROBE_REPLY: u64 = 64 * 1024;

/// The session server's "unauthorized" code.
const UNAUTHORIZED_CODE: i64 = -32002;

/// One probe exchange's outcome, before it is interpreted as a [`ServeProbe`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Reply {
    /// Nothing accepted the connection.
    Unreachable,
    /// Answered, but not as a session server.
    Foreign,
    /// A well-formed success response.
    Ok,
    /// A session server refusing the credentials presented.
    Unauthorized,
}

#[derive(Debug, thiserror::Error)]
pub enum LaneError {
    #[error("bind address {addr} is not resolvable: {reason}")]
    BadAddress { addr: String, reason: String },
    #[error(
        "refusing to use {addr}: the session server must be on loopback, because it can run \
         commands in your project"
    )]
    NotLoopback { addr: String },
    #[error("could not start `zo serve`: {0}")]
    StartServe(#[source] io::Error),
    #[error("`zo serve` did not answer the session protocol on {addr} within {waited:?}")]
    ServeNotReady { addr: String, waited: Duration },
    #[error(
        "something is listening on {addr} but it is not a session server — refusing to hand it \
         the token or attach to it"
    )]
    PortOccupied { addr: String },
    #[error(
        "a session server is running on {addr} but rejected our token; start ZeroCode with the \
         same {TOKEN_ENV} or stop that server"
    )]
    TokenMismatch { addr: String },
    #[error("the zo path is not valid UTF-8: {0}")]
    NonUtf8Path(PathBuf),
    #[error("the pane did not publish its events channel in {file} within {waited:?}")]
    PaneChannelNotReady { file: PathBuf, waited: Duration },
    /// The pane's process ended before it published — a `zo --resume` refused
    /// by the session's writer lease exits in well under a second, and waiting
    /// the rest of the patience out learned nothing (2026-09-13: 20 s of
    /// silence, then a refusal with no reason in it). `said` is the last line
    /// the process left on its terminal, which is where zo puts the reason.
    #[error("the pane exited (code {code}) before publishing its events channel{said}")]
    PaneExited { code: u32, said: String },
    #[error(transparent)]
    Pty(#[from] PtyTransportError),
    #[error(transparent)]
    TokenStorage(#[from] PlatformTokenError),
}

/// What is on the other end of the bind address.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServeProbe {
    /// Nothing accepted a connection.
    NotListening,
    /// Something accepted, but it does not answer the session protocol. Could
    /// be an unrelated service that happens to hold the port; the probe stops
    /// before sending that service the token.
    NotSessionServer,
    /// A session server, but it does not accept our token.
    Unauthorized,
    /// A session server that accepts our token.
    Ready,
}

/// What [`LaneSupervisor::ensure_serve`] had to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServeState {
    /// A server was already listening and accepted our token — possibly one the
    /// user started in their own terminal. Sharing it is the point: both sides
    /// then see the same sessions.
    AlreadyRunning,
    /// We started one and it answered the protocol.
    Started,
}

/// Runs `zo` for the IDE.
#[derive(Debug, Clone)]
pub struct LaneSupervisor {
    zo: ZoBinary,
    bind_addr: String,
    token: Option<String>,
}

/// A running IDE pane and the private events channel that belongs to it.
pub struct PaneProcess {
    pub pty: PtyHandle,
    pub addr: String,
}

impl LaneSupervisor {
    /// Build a supervisor for an explicit binary.
    ///
    /// Rejects a non-loopback bind address outright. There is no flag to
    /// override it: exposing a session server on a routable interface hands
    /// arbitrary command execution to the network, and no IDE convenience is
    /// worth offering that switch.
    pub fn new(
        zo: ZoBinary,
        bind_addr: impl Into<String>,
        token: Option<String>,
    ) -> Result<Self, LaneError> {
        let bind_addr = bind_addr.into();
        let bind_addr = validate_loopback_bind(&bind_addr)?.to_string();
        Ok(Self {
            zo,
            bind_addr,
            token,
        })
    }

    #[must_use]
    pub fn bind_addr(&self) -> &str {
        &self.bind_addr
    }

    #[must_use]
    pub fn zo_path(&self) -> &Path {
        &self.zo.path
    }

    #[must_use]
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    /// Check the bind address in two exchanges.
    ///
    /// A bare TCP connect is not enough: any process can hold a loopback port,
    /// and treating "something answered" as "the session server is up" would
    /// point a `zo attach` at whatever squatted there.
    ///
    /// The order matters as much as the check. The **first** exchange carries no
    /// token, so an unknown listener learns nothing but a method name; a real
    /// server either answers or refuses with `-32002`. Only then does the second
    /// exchange present the secret. This prevents accidental disclosure to an
    /// unrelated local service; it is not peer authentication. The product
    /// trusts every loopback listener on its single-user workstation.
    #[must_use]
    pub fn probe(&self) -> ServeProbe {
        let addr = resolve(&self.bind_addr).expect("constructor stores a numeric socket address");
        probe_session_server_with_token(addr, self.token.as_deref())
    }

    /// True only when a session server that accepts our token is listening.
    #[must_use]
    pub fn serve_is_up(&self) -> bool {
        self.probe() == ServeProbe::Ready
    }

    /// Make sure a usable session server is running, starting one if not.
    ///
    /// The started process is **detached**: its own process group (and session
    /// on unix), stdio to null, with the shared secret passed explicitly rather
    /// than inherited by luck. If the server were our child it would die with
    /// the IDE and take every lane with it.
    ///
    /// Readiness is confirmed by speaking the protocol, not by assuming the
    /// spawn worked, and an occupied-but-foreign port is an error rather than a
    /// silent attach attempt.
    pub fn ensure_serve(&self, ready_timeout: Duration) -> Result<ServeState, LaneError> {
        match self.probe() {
            ServeProbe::Ready => return Ok(ServeState::AlreadyRunning),
            ServeProbe::Unauthorized => {
                return Err(LaneError::TokenMismatch {
                    addr: self.bind_addr.clone(),
                });
            }
            ServeProbe::NotSessionServer => {
                return Err(LaneError::PortOccupied {
                    addr: self.bind_addr.clone(),
                });
            }
            ServeProbe::NotListening => {}
        }

        let mut command = Command::new(&self.zo.path);
        command
            .args(ZoBinary::serve_args(&self.bind_addr))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        self.apply_token(&mut command);
        detach(&mut command);
        // The reaping door rather than a bare spawn: the server is DETACHED —
        // its own process group — but it is still this process's child, and a
        // child nobody waits for stands in the process table as a zombie the
        // day it exits or crashes (zerocode_core::reap).
        zerocode_core::reap::spawn_forgotten(command).map_err(LaneError::StartServe)?;

        let deadline = Instant::now() + ready_timeout;
        while Instant::now() < deadline {
            match self.probe() {
                ServeProbe::Ready => return Ok(ServeState::Started),
                ServeProbe::Unauthorized => {
                    return Err(LaneError::TokenMismatch {
                        addr: self.bind_addr.clone(),
                    });
                }
                // Still booting: the port may be unbound or half-open.
                ServeProbe::NotListening | ServeProbe::NotSessionServer => {}
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        Err(LaneError::ServeNotReady {
            addr: self.bind_addr.clone(),
            waited: ready_timeout,
        })
    }

    /// Open a lane: `zo attach [session]` hosted in a pty.
    ///
    /// `session_id` is `None` to start a **new** session. The id is then
    /// omitted, because that is the only thing that makes the server create
    /// one; naming a session that does not exist yields a lane that renders
    /// fine and is owned by nothing.
    ///
    /// Closing the returned lane kills only this client. The session stays on
    /// the server, so calling this again with the same id reattaches to the work
    /// in progress — and so does a human typing `zo attach <id>` in their own
    /// terminal.
    ///
    /// Refuses to run when the address is not a session server that accepts our
    /// token: `zo attach` would send the secret as its first request, so a port
    /// squatter must never get that far.
    pub fn open_lane(
        &self,
        pty: Box<dyn PtySpawner>,
        session_id: Option<&str>,
        cwd: Option<PtyCwd>,
        env: &[(String, String)],
        rows: u16,
        cols: u16,
    ) -> Result<PtyHandle, LaneError> {
        match self.probe() {
            ServeProbe::Ready => {}
            ServeProbe::Unauthorized => {
                return Err(LaneError::TokenMismatch {
                    addr: self.bind_addr.clone(),
                });
            }
            ServeProbe::NotSessionServer => {
                return Err(LaneError::PortOccupied {
                    addr: self.bind_addr.clone(),
                });
            }
            ServeProbe::NotListening => {
                return Err(LaneError::ServeNotReady {
                    addr: self.bind_addr.clone(),
                    waited: Duration::ZERO,
                });
            }
        }
        self.spawn_attach(pty, session_id, cwd, env, rows, cols)
    }

    /// The pty spawn on its own, without the readiness gate. Exposed for the
    /// caller that has already probed (the app boots by probing once, then
    /// opening several lanes) — and used by [`Self::open_lane`].
    pub fn spawn_attach(
        &self,
        pty: Box<dyn PtySpawner>,
        session_id: Option<&str>,
        cwd: Option<PtyCwd>,
        env: &[(String, String)],
        rows: u16,
        cols: u16,
    ) -> Result<PtyHandle, LaneError> {
        let program = self
            .zo
            .path
            .to_str()
            .ok_or_else(|| LaneError::NonUtf8Path(self.zo.path.clone()))?;
        let args = ZoBinary::attach_args(session_id, &self.bind_addr);

        // The token travels explicitly, not by ambient inheritance: the lane
        // must reach the server we vouched for even when ZeroCode itself was
        // started without one in its environment.
        let mut env = env.to_vec();
        if let Some(token) = &self.token {
            env.retain(|(key, _)| key != TOKEN_ENV);
            env.push((TOKEN_ENV.to_string(), token.clone()));
        }
        let spec = match cwd {
            None => PtySpec::new(program, &args, None, &env, rows, cols),
            Some(PtyCwd::Local(path)) => {
                PtySpec::new(program, &args, Some(&path), &env, rows, cols)
            }
            Some(PtyCwd::Remote(path)) => PtySpec::remote(program, &args, path, &env, rows, cols),
        };
        Ok(pty.spawn(&spec)?)
    }

    /// Spawn one pane-owned `zo` and wait for its events channel address.
    ///
    /// This is intentionally separate from [`Self::open_lane`], which remains
    /// the legacy/shared-server road used by remote lanes. A local IDE pane
    /// starts no `zo serve` and sends no `session.create`; its own process owns
    /// the session, and the shell reads that id with `session.info` after this
    /// function returns its private channel address.
    #[allow(clippy::too_many_arguments)]
    pub fn open_pane(
        &self,
        pty: Box<dyn PtySpawner>,
        cwd: &Path,
        resume: Option<&str>,
        addr_file: &Path,
        env: &[(String, String)],
        extra_args: &[String],
        rows: u16,
        cols: u16,
        ready_timeout: Duration,
    ) -> Result<PaneProcess, LaneError> {
        let program = self
            .zo
            .path
            .to_str()
            .ok_or_else(|| LaneError::NonUtf8Path(self.zo.path.clone()))?;
        let mut args = ZoBinary::pane_args(PANE_CHANNEL_BIND, Some(cwd), resume);
        args.extend(pane_extra_args(extra_args));
        let mut env = env.to_vec();
        env.retain(|(key, _)| key != ZO_EVENTS_ADDR_FILE_ENV && key != TOKEN_ENV);
        env.push((
            ZO_EVENTS_ADDR_FILE_ENV.to_string(),
            addr_file.to_string_lossy().into_owned(),
        ));
        if let Some(token) = &self.token {
            env.push((TOKEN_ENV.to_string(), token.clone()));
        }

        // A previous process with the same lane id must not answer readiness
        // for this one. The file is also removed after every verdict below.
        let _ = std::fs::remove_file(addr_file);
        let spec = PtySpec::new(program, &args, Some(cwd), &env, rows, cols);
        let mut pty = pty.spawn(&spec)?;
        let waited = await_pane_channel_addr(&mut pty, addr_file, ready_timeout);
        let _ = std::fs::remove_file(addr_file);
        let addr = waited?;
        validate_loopback_bind(&addr)?;
        Ok(PaneProcess { pty, addr })
    }

    fn apply_token(&self, command: &mut Command) {
        if let Some(token) = &self.token {
            command.env(TOKEN_ENV, token);
        }
    }
}

/// Keep launch tuning while reserving the arguments that define an IDE pane.
///
/// A restored pane's session, working directory and channel address come from
/// durable/backend state. Letting a saved launch override repeat any of those
/// flags would create two competing answers on one command line — in
/// particular, a stale `--resume` could win over the session the durable
/// record asked to continue. Everything else (permission mode, model, effort)
/// rides through unchanged.
fn pane_extra_args(args: &[String]) -> Vec<String> {
    let mut kept = Vec::with_capacity(args.len());
    let mut at = 0;
    while at < args.len() {
        let arg = &args[at];
        if matches!(arg.as_str(), "--events-bind" | "--cwd" | "--resume") {
            at += 1;
            if at < args.len() && !args[at].starts_with('-') {
                at += 1;
            }
            continue;
        }
        if ["--events-bind=", "--cwd=", "--resume="]
            .iter()
            .any(|owned| arg.starts_with(owned))
        {
            at += 1;
            continue;
        }
        kept.push(arg.clone());
        at += 1;
    }
    kept
}

/// Wait until a pane has atomically completed its address-file line — or until
/// the pane's process has ended without one, whichever comes first.
fn await_pane_channel_addr(
    pty: &mut PtyHandle,
    addr_file: &Path,
    patience: Duration,
) -> Result<String, LaneError> {
    let deadline = Instant::now() + patience;
    loop {
        if let Ok(Some(identity)) = read_pane_channel(addr_file) {
            return Ok(identity.addr);
        }
        // A process that has ended will never publish. Its last words are the
        // reason, so they are read off its own terminal rather than guessed.
        if let Ok(Some(code)) = pty.try_wait() {
            // The file and the exit can land in the same instant: one more look
            // before calling it a refusal.
            if let Ok(Some(identity)) = read_pane_channel(addr_file) {
                return Ok(identity.addr);
            }
            return Err(LaneError::PaneExited {
                code,
                said: last_words_of(pty),
            });
        }
        if Instant::now() >= deadline {
            return Err(LaneError::PaneChannelNotReady {
                file: addr_file.to_path_buf(),
                waited: patience,
            });
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// The last non-empty line an ended pane left on its terminal, as the clause a
/// refusal carries (` — resume failed: …`), or nothing when it said nothing.
/// The output is pumped here because nobody else has read this pty yet.
fn last_words_of(pty: &mut PtyHandle) -> String {
    pty.pump();
    pty.terminal()
        .grid()
        .visible_text()
        .lines()
        .map(str::trim)
        .rfind(|line| !line.is_empty())
        .map(|line| format!(" — {}", line.chars().take(240).collect::<String>()))
        .unwrap_or_default()
}

/// Check that a loopback listener speaks the session protocol and accepts the
/// persisted token. The first exchange is deliberately unauthenticated, so an
/// unrelated local service never receives the bearer. The product's explicit
/// boundary is a single-user workstation where all loopback listeners are
/// trusted.
#[must_use]
pub fn probe_session_server(bind: SocketAddr, token: &str) -> ServeProbe {
    if !bind.ip().is_loopback() {
        return ServeProbe::NotSessionServer;
    }
    probe_session_server_with_token(bind, Some(token))
}

fn probe_session_server_with_token(bind: SocketAddr, token: Option<&str>) -> ServeProbe {
    let identity = match exchange(bind, None) {
        Ok(identity) => identity,
        Err(_) => return ServeProbe::NotListening,
    };
    match identity {
        Reply::Unreachable => ServeProbe::NotListening,
        Reply::Foreign => ServeProbe::NotSessionServer,
        Reply::Ok if token.is_none() => ServeProbe::Ready,
        Reply::Ok | Reply::Unauthorized => match exchange(bind, token) {
            Ok(Reply::Ok) => ServeProbe::Ready,
            Ok(Reply::Unauthorized) => ServeProbe::Unauthorized,
            Ok(Reply::Foreign) => ServeProbe::NotSessionServer,
            Ok(Reply::Unreachable) | Err(_) => ServeProbe::NotListening,
        },
    }
}

fn exchange(bind: SocketAddr, token: Option<&str>) -> io::Result<Reply> {
    let Ok(stream) = TcpStream::connect_timeout(&bind, PROBE_TIMEOUT) else {
        return Ok(Reply::Unreachable);
    };
    Ok(handshake(stream, token).unwrap_or(Reply::Foreign))
}

fn handshake(stream: TcpStream, token: Option<&str>) -> io::Result<Reply> {
    stream.set_read_timeout(Some(PROBE_TIMEOUT))?;
    stream.set_write_timeout(Some(PROBE_TIMEOUT))?;

    const PROBE_ID: u64 = 1;
    let mut request = serde_json::json!({
        "jsonrpc": "2.0",
        "id": PROBE_ID,
        "method": "session.list",
        "params": {},
    });
    if let Some(token) = token {
        request["token"] = serde_json::Value::String(token.to_string());
    }
    let mut line = serde_json::to_string(&request).map_err(io::Error::other)?;
    line.push('\n');

    let mut writer = &stream;
    writer.write_all(line.as_bytes())?;
    writer.flush()?;

    let mut reader = BufReader::new(&stream).take(MAX_PROBE_REPLY);
    let mut reply = String::new();
    if reader.read_line(&mut reply)? == 0 {
        return Ok(Reply::Foreign);
    }
    let Ok(value) = serde_json::from_str::<serde_json::Value>(reply.trim()) else {
        return Ok(Reply::Foreign);
    };
    if value.get("jsonrpc").is_none()
        || value.get("id").and_then(serde_json::Value::as_u64) != Some(PROBE_ID)
    {
        return Ok(Reply::Foreign);
    }
    if let Some(code) = value
        .get("error")
        .and_then(|error| error.get("code"))
        .and_then(serde_json::Value::as_i64)
    {
        return Ok(if code == UNAUTHORIZED_CODE {
            Reply::Unauthorized
        } else {
            Reply::Foreign
        });
    }
    Ok(if value.get("result").is_some() {
        Reply::Ok
    } else {
        Reply::Foreign
    })
}

/// Refuse any bind that could disclose the app-managed bearer to a remote
/// listener. Headless consumers such as `watch` use this gate without needing
/// to construct a supervisor or discover a `zo` binary.
pub fn validate_loopback_bind(bind: &str) -> Result<SocketAddr, LaneError> {
    let resolved = resolve(bind)?;
    if resolved.ip().is_loopback() {
        Ok(resolved)
    } else {
        Err(LaneError::NotLoopback {
            addr: bind.to_string(),
        })
    }
}

fn resolve(bind_addr: &str) -> Result<SocketAddr, LaneError> {
    bind_addr
        .parse::<SocketAddr>()
        .map_err(|error| LaneError::BadAddress {
            addr: bind_addr.to_string(),
            reason: error.to_string(),
        })
}

/// Mint a shared secret for a server we are about to start.
///
/// `RandomState` is seeded by the OS, so two of its hashers give 128 bits
/// without pulling in a random-number dependency for a loopback secret.
#[must_use]
pub fn generate_token() -> String {
    let high = RandomState::new().build_hasher().finish();
    let low = RandomState::new().build_hasher().finish();
    format!("{high:016x}{low:016x}")
}

/// Put the child in its own process group so it survives the IDE.
///
/// Without this the server shares our group, and a terminal-delivered signal or
/// the hangup when the IDE's own terminal closes takes it down with us — which
/// silently converts "detach" into "lose the session".
#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.process_group(0);
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    command.creation_flags(CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS);
}

#[cfg(not(any(unix, windows)))]
fn detach(_command: &mut Command) {}
