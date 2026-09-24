//! Worker-only Codex app-server sidecars and native pointer delivery.
//!
//! One orchestration worker gets one non-daemon app server on an explicit,
//! short Unix socket. Its TUI and `codex queue` both name that endpoint; no
//! shared daemon, default control socket, or undocumented RPC is involved.
//! Routes are process-local hints and carry only [`PointerNotice`].

use std::collections::HashMap;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use thiserror::Error;
use zerocode_core::ProviderSession;
use zerocode_core::provider_session::SessionKey;
use zerocode_hookd::session_notify::{NotificationOutcome, PointerNotice, SessionNotifier};

use crate::orchestration_notify::{RouteGeneration, RouteLease};

/// macOS `sockaddr_un.sun_path` holds 104 bytes including its trailing NUL.
/// The conservative bound also works on Unix targets with a wider field.
#[cfg(unix)]
const UNIX_SOCKET_PATH_MAX_BYTES: usize = 103;

/// Every bound one worker-launch preparation runs under, in one table.
///
/// The production numbers live here and nowhere else. A caller that does not
/// get the machine to itself — this module's own installed-CLI contract runs
/// beside eleven hundred other tests — names a wider budget instead of
/// keeping a second copy of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LaunchBudget {
    /// How long one capability `--help` probe may run before it counts as
    /// unanswered.
    probe_timeout: Duration,
    /// How many times ONE measurement re-asks a probe the machine did not
    /// answer. Distinct from the `attempts` an indeterminate answer carries,
    /// which counts measurements across time and widens the gap between them.
    probe_tries: u32,
    /// How long an unanswered launch environment waits before the next
    /// measurement.
    probe_retry_delay: Duration,
    /// How many times that wait may double before it stops growing.
    probe_retry_doublings: u32,
    /// The outer fence on app-server startup.
    sidecar_ready: Duration,
}

impl LaunchBudget {
    /// Help probes are local processes and should finish effectively at once.
    /// Two seconds tolerates a cold executable without letting capability
    /// detection stall worker launch indefinitely, and a worker launch asks
    /// once: patience belongs in the retry gap, not in a launch someone is
    /// waiting on.
    ///
    /// A probe that could not run is not evidence that the CLI surface is
    /// absent. Hold that indeterminate answer fifteen seconds so a burst of
    /// worker starts cannot amplify machine pressure, then let a later start
    /// measure again — every further answerless measurement doubling the
    /// wait, up to four doublings. A machine that has not recovered by then
    /// is not one a fifteen-second loop is helping, and the refusal carries
    /// the attempt count so a loop that is getting nowhere cannot keep
    /// reading like a first failure.
    ///
    /// App-server startup includes config and account loading. Five seconds
    /// is the outer launch fence; failure keeps the ordinary standalone TUI
    /// path.
    const PRODUCTION: Self = Self {
        probe_timeout: Duration::from_secs(2),
        probe_tries: 1,
        probe_retry_delay: Duration::from_secs(15),
        probe_retry_doublings: 4,
        sidecar_ready: Duration::from_secs(5),
    };

    /// Ask a machine that keeps failing to answer less and less often,
    /// without the silence ever hardening into a verdict about the CLI.
    fn retry_after(self, attempts: u32) -> Duration {
        self.probe_retry_delay
            * 2u32.saturating_pow(attempts.saturating_sub(1).min(self.probe_retry_doublings))
    }
}

/// Launch overrides can name different Codex executables, but a long-lived
/// window must not retain an unbounded path-keyed probe cache.
const CAPABILITY_CACHE_MAX: usize = 16;

/// Crash residue is bounded per boot. More owned routes remain for the next
/// restart instead of turning startup into an unbounded sequence of bounded
/// process terminations after an abnormal loop. The process table itself is
/// sampled once regardless of route count.
#[cfg(unix)]
const STALE_REAP_MAX: usize = 64;

/// A queue request is only a wake-up hint. Three seconds bounds the native
/// attempt before the common hub reconciles an unknown result through PTY.
const QUEUE_TIMEOUT: Duration = Duration::from_secs(3);

/// A retained Child handle is exact process authority. Give app-server two
/// seconds to honor TERM, then reap it with KILL instead of running repeated
/// process-table samplers on the terminal cleanup path.
const SIDECAR_TERM_TIMEOUT: Duration = Duration::from_secs(2);

/// Short polling keeps process and socket waits responsive without spinning.
const PROCESS_POLL_INTERVAL: Duration = Duration::from_millis(10);

/// Names only runtime directories this module owns beneath `/tmp`.
#[cfg(unix)]
const RUNTIME_DIR_PREFIX: &str = "zerocode-codex-route-v1-";

#[cfg(unix)]
const SOCKET_FILE: &str = "app.sock";

#[cfg(unix)]
const OWNER_FILE: &str = "owner.json";

#[cfg(unix)]
const OWNERSHIP_MARKER_HEX_LEN: usize = 32;

/// Only values needed to locate the CLI and its managed configuration cross
/// into a Codex process this module runs itself. Hook, team, browser, and
/// account credentials stay in the app-server/TUI environment where they
/// already belong.
///
/// The same two decide WHICH executable a bare command name reaches and which
/// managed home it reads, so they are also what a capability answer belongs
/// to. Measured: the four `--help` probes need nothing else, through the
/// mirror shim or straight at the installed binary.
const CODEX_ENV_ALLOWLIST: &[&str] = &["CODEX_HOME", "PATH"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CodexQueueCapability {
    Supported,
    /// The CLI ran and rejected one of the four surfaces.
    Unsupported,
    /// Nothing runnable answers to this command under this launch
    /// environment. Another worker's environment is another question and is
    /// measured on its own.
    Missing,
    /// No verdict: the machine, not the CLI, refused to answer.
    Indeterminate {
        stall: ProbeStall,
        attempts: u32,
    },
}

/// Why a capability probe produced no verdict.
///
/// A starved machine, a hung executable and a broken wait have three different
/// cures, and spending one word on all three is what sent a person hunting
/// through a CLI that was working the whole time.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProbeStall {
    Unspawnable,
    TimedOut,
    Unwaitable,
}

impl std::fmt::Display for ProbeStall {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(match self {
            Self::Unspawnable => "could not be started",
            Self::TimedOut => "did not answer inside its bound",
            Self::Unwaitable => "could not be waited on",
        })
    }
}

/// What one capability answer belongs to.
///
/// The command name alone is not the question. A summons names its agent by
/// the bare word it was written with, and which executable that word reaches
/// is decided by the launch environment — so an answer keyed on the name alone
/// lets one worker's environment answer for another's.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct CapabilityKey {
    program: PathBuf,
    env: Vec<(String, String)>,
}

#[derive(Debug, Clone, Copy)]
struct CachedCapability {
    capability: CodexQueueCapability,
    retry_at: Option<Instant>,
}

impl CachedCapability {
    fn current(self, now: Instant) -> Option<CodexQueueCapability> {
        self.retry_at
            .is_none_or(|retry_at| now < retry_at)
            .then_some(self.capability)
    }

    /// How many measurements in a row have left this key without a verdict.
    fn stalled_attempts(self) -> u32 {
        match self.capability {
            CodexQueueCapability::Indeterminate { attempts, .. } => attempts,
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CommandResult {
    /// Spawn was refused because nothing runnable answers to this program
    /// here — a fact about the launch, not about machine load.
    ProgramMissing,
    /// Spawn was refused for another reason. A machine out of process slots or
    /// memory recovers; a missing executable does not.
    SpawnFailed,
    Succeeded,
    Failed,
    TimedOut,
    WaitFailed,
}

impl CommandResult {
    /// Whether this is the MACHINE declining to answer rather than a verdict
    /// about the program. Only silence is worth re-asking.
    const fn is_stall(self) -> bool {
        matches!(self, Self::SpawnFailed | Self::TimedOut | Self::WaitFailed)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub(crate) enum CodexQueueError {
    #[cfg(not(unix))]
    #[error("this platform has no native Codex queue route")]
    UnsupportedPlatform,
    #[error("the installed Codex CLI has no compatible queue/app-server surface")]
    UnsupportedCli,
    #[error("no runnable Codex CLI answers to this worker's launch command on its launch PATH")]
    CliMissing,
    #[error(
        "the Codex CLI capability probe {stall} (attempt {attempts}); a later worker will retry"
    )]
    CapabilityIndeterminate { stall: ProbeStall, attempts: u32 },
    #[error("the worker launch already names another Codex remote")]
    ExistingRemote,
    /// A resumed thread keeps the PTY road (t-7812). Codex refuses a remote
    /// resume that carries a permission override — "Permission overrides
    /// are not supported when resuming a remote task." — and exits, which
    /// is how the ledger's reseat of w-7738 died 1.7 s in on 2026-09-25.
    #[error("a resumed Codex thread keeps the PTY road: Codex refuses a remote resume")]
    ResumedThread,
    #[error("a private short Codex runtime directory could not be created")]
    RuntimeDirectory,
    #[error("the Codex app-server sidecar could not be started")]
    SidecarSpawn,
    #[error("the Codex app-server process identity could not be established")]
    SidecarIdentity,
    #[error("the Codex app-server did not become ready inside its bound")]
    SidecarNotReady,
}

#[cfg(unix)]
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SidecarOwner {
    pid: u32,
    started: String,
    #[serde(default)]
    process_group: Option<u32>,
    #[serde(default)]
    creator_pid: Option<u32>,
    #[serde(default)]
    creator_started: Option<String>,
}

struct CodexSidecar {
    child: Child,
    started: String,
    creator_started: String,
    process_group: u32,
    runtime_dir: PathBuf,
    socket: PathBuf,
}

impl std::fmt::Debug for CodexSidecar {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexSidecar")
            .field("pid", &self.child.id())
            .field("process_group", &self.process_group)
            .field("started_bytes", &self.started.len())
            .field("creator_started_bytes", &self.creator_started.len())
            .field("runtime_dir", &"<redacted-short-runtime-dir>")
            .finish()
    }
}

impl CodexSidecar {
    fn stop(&mut self) {
        #[cfg(unix)]
        self.stop_process_group();
        #[cfg(not(unix))]
        self.stop_child();
        let _ = self.child.wait();
        cleanup_runtime_dir(&self.runtime_dir, &self.socket);
    }

    #[cfg(unix)]
    fn stop_process_group(&mut self) {
        let leader_is_live = match self.child.try_wait() {
            Ok(None) => true,
            Ok(Some(_)) => false,
            Err(error) => {
                eprintln!(
                    "zerocode-shell: Codex sidecar leader {} could not be waited on: {error}",
                    self.child.id()
                );
                false
            }
        };
        let group_is_owned = leader_is_live
            || process_group_has_route(self.process_group, &self.socket).unwrap_or_else(|error| {
                eprintln!(
                    "zerocode-shell: Codex sidecar process group {} could not be verified after its leader exited: {error}",
                    self.process_group
                );
                false
            });
        if !group_is_owned {
            return;
        }
        if let Err(error) = signal_process_group(self.process_group, libc::SIGTERM)
            && error.raw_os_error() != Some(libc::ESRCH)
        {
            eprintln!(
                "zerocode-shell: Codex sidecar process group {} could not receive TERM: {error}",
                self.process_group
            );
        }
        let deadline = Instant::now() + SIDECAR_TERM_TIMEOUT;
        while process_group_exists(self.process_group) && Instant::now() < deadline {
            let _ = self.child.try_wait();
            std::thread::sleep(PROCESS_POLL_INTERVAL);
        }
        if process_group_exists(self.process_group) {
            if let Err(error) = signal_process_group(self.process_group, libc::SIGKILL)
                && error.raw_os_error() != Some(libc::ESRCH)
            {
                eprintln!(
                    "zerocode-shell: Codex sidecar process group {} could not receive KILL: {error}",
                    self.process_group
                );
            }
            let deadline = Instant::now() + SIDECAR_TERM_TIMEOUT;
            while process_group_exists(self.process_group) && Instant::now() < deadline {
                let _ = self.child.try_wait();
                std::thread::sleep(PROCESS_POLL_INTERVAL);
            }
            if process_group_exists(self.process_group) {
                eprintln!(
                    "zerocode-shell: Codex sidecar process group {} survived KILL",
                    self.process_group
                );
            }
        }
    }

    #[cfg(not(unix))]
    fn stop_child(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
    }
}

#[cfg(unix)]
fn prepare_process_group(command: &mut std::process::Command) {
    use std::os::unix::process::CommandExt as _;

    command.process_group(0);
}

#[cfg(unix)]
fn signal_process_group(process_group: u32, signal: libc::c_int) -> std::io::Result<()> {
    let process_group = libc::pid_t::try_from(process_group)
        .map_err(|_| std::io::Error::other("process group id is outside pid_t"))?;
    // SAFETY: the negative, non-zero pid addresses only the fresh process
    // group created for this sidecar. No pointer crosses the FFI boundary.
    if unsafe { libc::kill(-process_group, signal) } == 0 {
        Ok(())
    } else {
        Err(std::io::Error::last_os_error())
    }
}

#[cfg(unix)]
fn process_group_exists(process_group: u32) -> bool {
    match signal_process_group(process_group, 0) {
        Ok(()) => true,
        Err(error) => error.raw_os_error() == Some(libc::EPERM),
    }
}

#[cfg(unix)]
fn process_group_has_route(process_group: u32, socket: &Path) -> Result<bool, String> {
    let uid = unsafe { libc::getuid() };
    let endpoint = format!("unix://{}", socket.to_string_lossy());
    let sample = crate::resource_usage::enumerate_processes()?;
    Ok(sample
        .owned_processes_with_args(uid, &["app-server", "--listen", &endpoint])
        .into_iter()
        .any(|process| process.process_group == process_group))
}

impl Drop for CodexSidecar {
    fn drop(&mut self) {
        self.stop();
    }
}

struct CodexNotifier {
    program: PathBuf,
    cwd: PathBuf,
    endpoint: String,
    socket: PathBuf,
    queue_env: Vec<(String, String)>,
    thread_id: Mutex<Option<String>>,
}

impl std::fmt::Debug for CodexNotifier {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let thread_bound = self
            .thread_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .is_some();
        let queue_env_keys: Vec<&str> = self
            .queue_env
            .iter()
            .map(|(name, _)| name.as_str())
            .collect();
        formatter
            .debug_struct("CodexNotifier")
            .field("program", &"<redacted-program>")
            .field("cwd", &"<redacted-worktree>")
            .field("endpoint", &"<redacted-unix-endpoint>")
            .field("queue_env_keys", &queue_env_keys)
            .field("thread_bound", &thread_bound)
            .finish()
    }
}

impl SessionNotifier for CodexNotifier {
    fn notify(&self, notice: &PointerNotice) -> NotificationOutcome {
        let thread_id = self
            .thread_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        let Some(thread_id) = thread_id else {
            return NotificationOutcome::DefinitelyUnsent;
        };
        #[cfg(unix)]
        {
            use std::os::unix::net::UnixStream;

            if UnixStream::connect(&self.socket).is_err() {
                return NotificationOutcome::DefinitelyUnsent;
            }
        }
        #[cfg(not(unix))]
        {
            return NotificationOutcome::DefinitelyUnsent;
        }
        let args = queue_args(&self.endpoint, &thread_id, notice.text());
        let result = run_bounded_command(
            &self.program,
            &args,
            Some(&self.cwd),
            Some(&self.queue_env),
            QUEUE_TIMEOUT,
        );
        classify_queue_result(result)
    }
}

/// Sidecar prepared before the worker PTY exists.
///
/// Dropping it rolls the process and socket back. `commit` is the only road
/// that registers a native notification route, and is called only after PTY
/// spawn succeeds.
pub(crate) struct PendingCodexRoute {
    sidecar: Option<CodexSidecar>,
    notifier: Arc<CodexNotifier>,
}

impl std::fmt::Debug for PendingCodexRoute {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PendingCodexRoute")
            .field("sidecar", &self.sidecar)
            .field("notifier", &self.notifier)
            .finish()
    }
}

impl PendingCodexRoute {
    pub(crate) fn commit(mut self, term: u32) {
        let Some(sidecar) = self.sidecar.take() else {
            return;
        };
        let notifier: Arc<dyn SessionNotifier> = self.notifier.clone();
        let lease = crate::orchestration_notify::register_route(term, notifier);
        let generation = lease.generation();
        let owner = CodexRouteOwner {
            sidecar,
            notifier: self.notifier.clone(),
            _lease: lease,
            generation,
        };
        let replaced = routes()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(term, owner);
        if let Some(replaced) = replaced {
            retire_route(replaced);
        }
    }
}

struct CodexRouteOwner {
    sidecar: CodexSidecar,
    notifier: Arc<CodexNotifier>,
    _lease: RouteLease,
    generation: RouteGeneration,
}

impl std::fmt::Debug for CodexRouteOwner {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexRouteOwner")
            .field("sidecar", &self.sidecar)
            .field("notifier", &self.notifier)
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

fn routes() -> &'static Mutex<HashMap<u32, CodexRouteOwner>> {
    static ROUTES: OnceLock<Mutex<HashMap<u32, CodexRouteOwner>>> = OnceLock::new();
    ROUTES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The vendor binary behind this window's own mirror shims.
///
/// Those shims sit at the head of every worker's `PATH`, and the mirror
/// behind them refuses a run that is piped AND sitting in an orchestration
/// seat — the shape of an agent quietly starting a nested agent. A sidecar is
/// the opposite of that: the window starts it, its stdio is null because it
/// speaks over a socket, and it carries the worker's seat only because it
/// inherits the worker's environment. Seen from the guard those two are
/// identical, so the sidecar was refused in 30ms and the window reported the
/// only thing it could see — a server that never became ready (measured:
/// through the shim, exit 2 in 0.03s; the same argv on the real binary
/// answers its socket in 0.12s).
///
/// Resolving past the shims fixes that without opening a hole for anyone
/// else, and takes the mirror out of a transport whose bytes it cannot tee
/// anyway. It also removes the extra process that made these sidecars
/// outlive their window in the first place: the shim was the child the
/// window ended, and the app-server was the grandchild that survived.
///
/// Only a bare program name is rewritten. An absolute path never consults
/// `PATH`, so leaving it alone is both correct and the smaller promise.
#[cfg(unix)]
fn past_the_shims(program: &Path, env: &[(String, String)]) -> PathBuf {
    past_the_shims_in(
        program,
        env.iter()
            .find(|(name, _)| name == "PATH")
            .map(|(_, path)| std::ffi::OsString::from(path))
            .as_deref(),
        crate::hooks::mirror_shims()
            .map(|(shims, _)| shims)
            .as_deref(),
    )
}

/// The walk itself, over one `PATH` and one shim directory.
///
/// Split out for the same reason the shim installer splits its own: a
/// decision this quiet needs to be checkable without moving the running
/// process's environment underneath every other test in the file.
fn past_the_shims_in(
    program: &Path,
    path: Option<&std::ffi::OsStr>,
    shims: Option<&Path>,
) -> PathBuf {
    let unchanged = || program.to_path_buf();
    if program.components().count() != 1 {
        return unchanged();
    }
    let (Some(name), Some(shims), Some(path)) = (
        program.file_name().and_then(std::ffi::OsStr::to_str),
        shims,
        path,
    ) else {
        return unchanged();
    };
    // Only when the shim is what would actually run. A `PATH` that never had
    // one, or has already been walked past, is resolving the real binary.
    if zerocode_core::agent::resolve_on_path(Some(path), name).as_deref()
        != Some(shims.join(name).as_path())
    {
        return unchanged();
    }
    let kept = std::env::split_paths(path).filter(|dir| dir != shims);
    let Ok(without) = std::env::join_paths(kept) else {
        return unchanged();
    };
    zerocode_core::agent::resolve_on_path(Some(&without), name).unwrap_or_else(unchanged)
}

/// Start a worker sidecar and prepend its explicit endpoint to fresh or resume
/// Codex argv. On every refusal `args` is left byte-for-byte unchanged.
pub(crate) fn prepare(
    program: &Path,
    args: &mut Vec<String>,
    cwd: &Path,
    env: &[(String, String)],
) -> Result<PendingCodexRoute, CodexQueueError> {
    prepare_with(program, args, cwd, env, LaunchBudget::PRODUCTION)
}

/// The same road with its bounds named, so a caller that shares its machine
/// can widen them without holding a second copy of the production table.
fn prepare_with(
    program: &Path,
    args: &mut Vec<String>,
    cwd: &Path,
    env: &[(String, String)],
    budget: LaunchBudget,
) -> Result<PendingCodexRoute, CodexQueueError> {
    #[cfg(not(unix))]
    {
        let _ = (program, args, cwd, env, budget);
        return Err(CodexQueueError::UnsupportedPlatform);
    }
    #[cfg(unix)]
    {
        if has_remote(args) {
            return Err(CodexQueueError::ExistingRemote);
        }
        if resumes_a_thread(args) {
            return Err(CodexQueueError::ResumedThread);
        }
        let vendor = past_the_shims(program, env);
        match capability(&vendor, env, budget) {
            CodexQueueCapability::Supported => {}
            CodexQueueCapability::Unsupported => return Err(CodexQueueError::UnsupportedCli),
            CodexQueueCapability::Missing => return Err(CodexQueueError::CliMissing),
            CodexQueueCapability::Indeterminate { stall, attempts } => {
                return Err(CodexQueueError::CapabilityIndeterminate { stall, attempts });
            }
        }
        let (runtime_dir, socket, endpoint) = create_runtime_dir()?;
        let sidecar = start_sidecar(&vendor, cwd, env, runtime_dir, socket, &endpoint, budget)?;
        let route_socket = sidecar.socket.clone();
        prepend_remote(args, &endpoint);
        Ok(PendingCodexRoute {
            sidecar: Some(sidecar),
            notifier: Arc::new(CodexNotifier {
                program: program.to_path_buf(),
                cwd: cwd.to_path_buf(),
                endpoint,
                socket: route_socket,
                queue_env: minimal_codex_env(env),
                thread_id: Mutex::new(None),
            }),
        })
    }
}

/// Bind the hook-authenticated Codex thread to the current route generation.
pub(crate) fn bind_session(term: u32, session: &ProviderSession) {
    if session.key != SessionKey::SessionId
        || !zerocode_core::provider_session::is_usable_session_id(&session.id)
    {
        return;
    }
    let held = routes()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(route) = held.get(&term) {
        *route
            .notifier
            .thread_id
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(session.id.clone());
    }
}

/// Remove one route immediately and stop its sidecar off the terminal cleanup
/// road so a reluctant process cannot stall pane teardown.
pub(crate) fn forget_term(term: u32) {
    let removed = routes()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .remove(&term);
    if let Some(removed) = removed {
        retire_route(removed);
    }
}

fn retire_route(owner: CodexRouteOwner) {
    let CodexRouteOwner {
        sidecar,
        notifier: _,
        _lease: lease,
        generation: _,
    } = owner;
    drop(lease);
    let _ = std::thread::Builder::new()
        .name("zerocode-codex-sidecar-stop".to_string())
        .spawn(move || drop(sidecar));
}

/// Stop every live sidecar before the application process exits.
pub(crate) fn shutdown_all() {
    let owners: Vec<CodexRouteOwner> = routes()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .drain()
        .map(|(_, owner)| owner)
        .collect();
    let mut sidecars = Vec::with_capacity(owners.len());
    for owner in owners {
        let CodexRouteOwner {
            sidecar,
            notifier: _,
            _lease: lease,
            generation: _,
        } = owner;
        drop(lease);
        sidecars.push(sidecar);
    }
    std::thread::scope(|scope| {
        for sidecar in sidecars {
            scope.spawn(move || drop(sidecar));
        }
    });
}

fn has_remote(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "--remote" || arg.starts_with("--remote="))
}

/// Whether this launch line re-enters an existing thread (`codex … resume
/// <id>`, the core's own spelling of it).
fn resumes_a_thread(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == zerocode_core::provider_session::CODEX_RESUME_SUBCOMMAND)
}

impl CodexQueueError {
    /// Whether this refusal leaves a worker with less than it was meant to
    /// have — worth the window's warning — or is the route working as
    /// designed: a resumed thread takes the PTY road on purpose (t-7812), and
    /// warning about it would cry wolf at every restart.
    pub(crate) fn degrades(self) -> bool {
        !matches!(self, Self::ResumedThread)
    }
}

fn prepend_remote(args: &mut Vec<String>, endpoint: &str) {
    args.insert(0, endpoint.to_string());
    args.insert(0, "--remote".to_string());
}

fn queue_args(endpoint: &str, thread_id: &str, pointer: &str) -> Vec<String> {
    vec![
        "queue".to_string(),
        "--remote".to_string(),
        endpoint.to_string(),
        "--thread".to_string(),
        thread_id.to_string(),
        "--message".to_string(),
        pointer.to_string(),
    ]
}

fn minimal_codex_env(env: &[(String, String)]) -> Vec<(String, String)> {
    CODEX_ENV_ALLOWLIST
        .iter()
        .filter_map(|wanted| env.iter().rev().find(|(name, _)| name == wanted).cloned())
        .collect()
}

fn classify_queue_result(result: CommandResult) -> NotificationOutcome {
    match result {
        CommandResult::ProgramMissing | CommandResult::SpawnFailed => {
            NotificationOutcome::DefinitelyUnsent
        }
        CommandResult::Succeeded => NotificationOutcome::Confirmed,
        CommandResult::Failed | CommandResult::TimedOut | CommandResult::WaitFailed => {
            NotificationOutcome::Unknown
        }
    }
}

/// Whether the CLI a worker is ABOUT TO LAUNCH carries the queue and
/// app-server surfaces.
///
/// The probe runs under the worker's own `PATH` and `CODEX_HOME`, never the
/// window's. Those two are the whole reason this function once answered for a
/// program nobody was going to run: a window opened from the dock inherits
/// `PATH=/usr/bin:/bin:/usr/sbin:/sbin`, a summons names its agent by the bare
/// word `codex`, and probing under the inherited environment could not start
/// anything at all. Every Codex worker on a machine with a working, installed
/// CLI fell to PTY, and the fifteen-second retry re-measured the one PATH that
/// was never going to change.
fn capability(
    program: &Path,
    env: &[(String, String)],
    budget: LaunchBudget,
) -> CodexQueueCapability {
    static CACHE: OnceLock<Mutex<HashMap<CapabilityKey, CachedCapability>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let key = CapabilityKey {
        program: program.to_path_buf(),
        env: minimal_codex_env(env),
    };
    capability_with(
        &key,
        cache,
        Instant::now(),
        budget,
        |program, args, timeout| run_bounded_command(program, args, None, Some(&key.env), timeout),
    )
}

/// One more measurement that left the surface unmeasured.
fn unmeasured(stall: ProbeStall, stalled: u32) -> CodexQueueCapability {
    CodexQueueCapability::Indeterminate {
        stall,
        attempts: stalled.saturating_add(1),
    }
}

/// Ask one surface, and re-ask while it is the machine that is not
/// answering. A verdict — the CLI saying yes or no — is taken the first time
/// it is given, and the try count is a bound rather than a loop.
fn measure(
    run_probe: &mut impl FnMut(&Path, &[String], Duration) -> CommandResult,
    program: &Path,
    args: &[String],
    budget: LaunchBudget,
) -> CommandResult {
    let mut result = run_probe(program, args, budget.probe_timeout);
    for _ in 1..budget.probe_tries {
        if !result.is_stall() {
            break;
        }
        result = run_probe(program, args, budget.probe_timeout);
    }
    result
}

fn capability_with(
    key: &CapabilityKey,
    cache: &Mutex<HashMap<CapabilityKey, CachedCapability>>,
    now: Instant,
    budget: LaunchBudget,
    mut run_probe: impl FnMut(&Path, &[String], Duration) -> CommandResult,
) -> CodexQueueCapability {
    let cached = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .get(key)
        .copied();
    if let Some(found) = cached.and_then(|held| held.current(now)) {
        return found;
    }
    let dummy = "unix:///tmp/zerocode-codex-capability.sock";
    let probes: &[&[&str]] = &[
        &["app-server", "--listen", dummy, "--help"],
        &[
            "queue",
            "--remote",
            dummy,
            "--thread",
            "probe",
            "--message",
            "probe",
            "--help",
        ],
        &["--remote", dummy, "--help"],
        &["resume", "--remote", dummy, "--help"],
    ];
    let stalled = cached.map_or(0, CachedCapability::stalled_attempts);
    let mut found = CodexQueueCapability::Supported;
    for args in probes {
        let args = args
            .iter()
            .map(|arg| (*arg).to_string())
            .collect::<Vec<_>>();
        found = match measure(&mut run_probe, &key.program, &args, budget) {
            CommandResult::Succeeded => continue,
            CommandResult::Failed => CodexQueueCapability::Unsupported,
            CommandResult::ProgramMissing => CodexQueueCapability::Missing,
            CommandResult::SpawnFailed => unmeasured(ProbeStall::Unspawnable, stalled),
            CommandResult::TimedOut => unmeasured(ProbeStall::TimedOut, stalled),
            CommandResult::WaitFailed => unmeasured(ProbeStall::Unwaitable, stalled),
        };
        break;
    }
    // A measured answer stands for this key. `Missing` and `Unsupported` are
    // both the machine answering plainly, and re-asking them every fifteen
    // seconds only turns one fact into a stream of identical toasts; a worker
    // whose launch environment differs is a different key and is measured
    // afresh. Only silence gets a deadline, and each silence a longer one.
    let retry_at = match found {
        CodexQueueCapability::Indeterminate { attempts, .. } => {
            Some(now + budget.retry_after(attempts))
        }
        _ => None,
    };
    let mut cache = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if cache.len() >= CAPABILITY_CACHE_MAX && !cache.contains_key(key) {
        cache.clear();
    }
    cache.insert(
        key.clone(),
        CachedCapability {
            capability: found,
            retry_at,
        },
    );
    found
}

fn run_bounded_command(
    program: &Path,
    args: &[String],
    cwd: Option<&Path>,
    env: Option<&[(String, String)]>,
    timeout: Duration,
) -> CommandResult {
    let mut command = crate::proc::quiet_command(program);
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    if let Some(env) = env {
        command
            .env_clear()
            .envs(env.iter().map(|(name, value)| (name, value)));
    }
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => return spawn_result(error.kind()),
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return command_result(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(PROCESS_POLL_INTERVAL);
            }
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return CommandResult::TimedOut;
            }
            Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return CommandResult::WaitFailed;
            }
        }
    }
}

/// A spawn refusal that names a missing or unusable executable is a fact about
/// the launch. Everything else — an exhausted process table, a machine out of
/// memory — is a state the machine can leave on its own.
fn spawn_result(kind: std::io::ErrorKind) -> CommandResult {
    match kind {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied => {
            CommandResult::ProgramMissing
        }
        _ => CommandResult::SpawnFailed,
    }
}

fn command_result(status: ExitStatus) -> CommandResult {
    if status.success() {
        CommandResult::Succeeded
    } else {
        CommandResult::Failed
    }
}

#[cfg(unix)]
fn create_runtime_dir() -> Result<(PathBuf, PathBuf, String), CodexQueueError> {
    create_runtime_dir_in(Path::new("/tmp"))
}

#[cfg(unix)]
fn create_runtime_dir_in(base: &Path) -> Result<(PathBuf, PathBuf, String), CodexQueueError> {
    use std::os::unix::ffi::OsStrExt as _;
    use std::os::unix::fs::DirBuilderExt as _;

    // `/tmp` is intentional: the managed CODEX_HOME path on macOS is already
    // too close to `sun_path[104]` for a control-socket suffix.
    let uid = unsafe { libc::getuid() };
    for _ in 0..32 {
        // This marker remains in the app-server argv after an external `/tmp`
        // cleanup removes the directory. Unlike the old per-process counter,
        // it is strong native-authored evidence when recovery has only the
        // process table left.
        let ownership_high = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        let ownership_low = std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish();
        let runtime_dir = base.join(format!(
            "{RUNTIME_DIR_PREFIX}{uid}-{}-{ownership_high:016x}{ownership_low:016x}",
            std::process::id()
        ));
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        match builder.create(&runtime_dir) {
            Ok(()) => {
                let socket = runtime_dir.join(SOCKET_FILE);
                if socket.as_os_str().as_bytes().len() > UNIX_SOCKET_PATH_MAX_BYTES {
                    cleanup_runtime_dir(&runtime_dir, &socket);
                    return Err(CodexQueueError::RuntimeDirectory);
                }
                let endpoint = format!("unix://{}", socket.to_string_lossy());
                return Ok((runtime_dir, socket, endpoint));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return Err(CodexQueueError::RuntimeDirectory),
        }
    }
    Err(CodexQueueError::RuntimeDirectory)
}

#[cfg(unix)]
fn start_sidecar(
    program: &Path,
    cwd: &Path,
    env: &[(String, String)],
    runtime_dir: PathBuf,
    socket: PathBuf,
    endpoint: &str,
    budget: LaunchBudget,
) -> Result<CodexSidecar, CodexQueueError> {
    let creator_started = crate::resource_usage::process_start_identity(std::process::id())
        .map_err(|_| CodexQueueError::SidecarIdentity)?;
    let mut command = crate::proc::quiet_command(program);
    command
        .args(["app-server", "--listen", endpoint])
        .current_dir(cwd)
        .env_clear()
        .envs(env.iter().map(|(name, value)| (name, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    prepare_process_group(&mut command);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(_) => {
            cleanup_runtime_dir(&runtime_dir, &socket);
            return Err(CodexQueueError::SidecarSpawn);
        }
    };
    let started = match crate::resource_usage::process_start_identity(child.id()) {
        Ok(started) => started,
        Err(_) => {
            let _ = signal_process_group(child.id(), libc::SIGKILL);
            let _ = child.kill();
            let _ = child.wait();
            cleanup_runtime_dir(&runtime_dir, &socket);
            return Err(CodexQueueError::SidecarIdentity);
        }
    };
    let mut sidecar = CodexSidecar {
        process_group: child.id(),
        child,
        started,
        creator_started,
        runtime_dir,
        socket,
    };
    if write_owner(&sidecar).is_err() {
        return Err(CodexQueueError::RuntimeDirectory);
    }
    let deadline = Instant::now() + budget.sidecar_ready;
    loop {
        if std::os::unix::net::UnixStream::connect(&sidecar.socket).is_ok() {
            return Ok(sidecar);
        }
        if sidecar.child.try_wait().ok().flatten().is_some() || Instant::now() >= deadline {
            return Err(CodexQueueError::SidecarNotReady);
        }
        std::thread::sleep(PROCESS_POLL_INTERVAL);
    }
}

#[cfg(unix)]
fn write_owner(sidecar: &CodexSidecar) -> Result<(), ()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;

    let bytes = serde_json::to_vec(&SidecarOwner {
        pid: sidecar.child.id(),
        started: sidecar.started.clone(),
        process_group: Some(sidecar.process_group),
        creator_pid: Some(std::process::id()),
        creator_started: Some(sidecar.creator_started.clone()),
    })
    .map_err(|_| ())?;
    let path = sidecar.runtime_dir.join(OWNER_FILE);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    let mut file = options.open(path).map_err(|_| ())?;
    file.write_all(&bytes).map_err(|_| ())?;
    file.sync_all().map_err(|_| ())
}

#[cfg(unix)]
fn cleanup_runtime_dir(runtime_dir: &Path, socket: &Path) {
    let _ = std::fs::remove_file(socket);
    let _ = std::fs::remove_file(runtime_dir.join(OWNER_FILE));
    let _ = std::fs::remove_dir(runtime_dir);
}

#[cfg(not(unix))]
fn cleanup_runtime_dir(_runtime_dir: &Path, _socket: &Path) {}

/// Reap only sidecars whose persisted pid, start identity, and exact app-server
/// argv still agree. A reused pid or an unfamiliar process is left untouched.
#[cfg(unix)]
pub(crate) fn reap_stale() -> usize {
    reap_stale_in(Path::new("/tmp"))
}

#[cfg(unix)]
#[derive(Debug, Clone)]
struct OwnedRuntimeRoute {
    runtime_dir: PathBuf,
    endpoint: String,
    creator_pid: u32,
}

#[cfg(unix)]
fn owned_runtime_route(base: &Path, uid: u32, endpoint: &str) -> Option<OwnedRuntimeRoute> {
    let socket = PathBuf::from(endpoint.strip_prefix("unix://")?);
    let runtime_dir = socket.parent()?.to_path_buf();
    if runtime_dir.parent()? != base || socket != runtime_dir.join(SOCKET_FILE) {
        return None;
    }
    let name = runtime_dir.file_name()?.to_str()?;
    let suffix = name.strip_prefix(&format!("{RUNTIME_DIR_PREFIX}{uid}-"))?;
    let (creator, ownership) = suffix.split_once('-')?;
    let creator_pid = creator.parse::<u32>().ok().filter(|pid| *pid != 0)?;
    let legacy_counter = ownership.parse::<u64>().is_ok();
    let random_marker = ownership.len() == OWNERSHIP_MARKER_HEX_LEN
        && ownership.bytes().all(|byte| byte.is_ascii_hexdigit());
    if !legacy_counter && !random_marker {
        return None;
    }
    let expected = format!("unix://{}", socket.to_string_lossy());
    (expected == endpoint).then(|| OwnedRuntimeRoute {
        runtime_dir,
        endpoint: endpoint.to_string(),
        creator_pid,
    })
}

#[cfg(unix)]
fn listened_endpoint(command: &str) -> Option<&str> {
    let words = command
        .split_ascii_whitespace()
        .map(|word| word.trim_matches(['\'', '"']))
        .collect::<Vec<_>>();
    words
        .windows(2)
        .find(|pair| pair[0] == "--listen")
        .map(|pair| pair[1])
}

#[cfg(unix)]
fn matching_sidecars(
    base: &Path,
    uid: u32,
    sample: &crate::resource_usage::ProcessSample,
) -> HashMap<
    String,
    (
        OwnedRuntimeRoute,
        Vec<crate::resource_usage::OwnedProcessMatch>,
    ),
> {
    let mut routes = HashMap::new();
    for process in sample.owned_processes_with_args(uid, &["app-server", "--listen"]) {
        let Some(endpoint) = listened_endpoint(&process.command) else {
            continue;
        };
        let Some(route) = owned_runtime_route(base, uid, endpoint) else {
            continue;
        };
        routes
            .entry(route.endpoint.clone())
            .or_insert_with(|| (route, Vec::new()))
            .1
            .push(process);
    }
    routes
}

#[cfg(unix)]
fn stop_matching_sidecars(
    endpoint: &str,
    processes: &mut [crate::resource_usage::OwnedProcessMatch],
) -> bool {
    // Stop wrappers before the children they wait on. Every pid is still
    // independently checked against its sampled start identity immediately
    // before TERM, so a concurrent pid reuse is never termination authority.
    let pids = processes
        .iter()
        .map(|process| process.pid)
        .collect::<Vec<_>>();
    processes.sort_by_key(|process| (u8::from(pids.contains(&process.parent_pid)), process.pid));
    let mut stopped = true;
    for process in processes {
        if crate::resource_usage::process_start_identity(process.pid).as_deref()
            != Ok(process.started.as_str())
        {
            continue;
        }
        if !crate::resource_usage::terminate_process(process.pid, &process.started) {
            eprintln!(
                "zerocode-shell: stale Codex sidecar pid {} for {endpoint} could not be stopped",
                process.pid
            );
            stopped = false;
        }
    }
    stopped
}

#[cfg(unix)]
fn say_reap_refusal(runtime_dir: &Path, reason: &str) {
    eprintln!(
        "zerocode-shell: stale Codex route {} was not reaped: {reason}",
        runtime_dir.to_string_lossy()
    );
}

#[cfg(unix)]
fn reap_stale_in(base: &Path) -> usize {
    use std::os::unix::fs::FileTypeExt as _;
    use std::os::unix::fs::MetadataExt as _;

    let uid = unsafe { libc::getuid() };
    let prefix = format!("{RUNTIME_DIR_PREFIX}{uid}-");
    let sample = match crate::resource_usage::enumerate_processes() {
        Ok(sample) => sample,
        Err(error) => {
            eprintln!(
                "zerocode-shell: stale Codex sidecars were not reaped because the process table could not be read: {error}"
            );
            return 0;
        }
    };
    let mut process_routes = matching_sidecars(base, uid, &sample);
    let entries = match std::fs::read_dir(base) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!(
                "zerocode-shell: stale Codex runtime directories under {} could not be read: {error}",
                base.to_string_lossy()
            );
            return 0;
        }
    };
    let mut reaped = 0;
    let mut inspected = 0;
    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                eprintln!(
                    "zerocode-shell: one stale Codex runtime entry could not be read: {error}"
                );
                continue;
            }
        };
        let name = entry.file_name();
        let Some(_) = name.to_str().filter(|name| name.starts_with(&prefix)) else {
            continue;
        };
        let runtime_dir = entry.path();
        let metadata = match std::fs::symlink_metadata(&runtime_dir) {
            Ok(metadata) => metadata,
            Err(error) => {
                say_reap_refusal(
                    &runtime_dir,
                    &format!("its metadata could not be read: {error}"),
                );
                continue;
            }
        };
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != uid
            || metadata.mode() & 0o077 != 0
        {
            say_reap_refusal(
                &runtime_dir,
                "the runtime directory is not a private owned directory",
            );
            continue;
        }
        inspected += 1;
        if inspected > STALE_REAP_MAX {
            eprintln!(
                "zerocode-shell: stale Codex reap stopped after its {STALE_REAP_MAX}-route startup bound"
            );
            break;
        }
        let owner_path = runtime_dir.join(OWNER_FILE);
        let owner_metadata = match std::fs::symlink_metadata(&owner_path) {
            Ok(metadata) => metadata,
            Err(error) => {
                say_reap_refusal(
                    &runtime_dir,
                    &format!("owner evidence could not be read: {error}"),
                );
                continue;
            }
        };
        if !owner_metadata.is_file()
            || owner_metadata.file_type().is_symlink()
            || owner_metadata.uid() != uid
            || owner_metadata.mode() & 0o077 != 0
        {
            say_reap_refusal(&runtime_dir, "owner evidence is not a private owned file");
            continue;
        }
        let bytes = match std::fs::read(&owner_path) {
            Ok(bytes) => bytes,
            Err(error) => {
                say_reap_refusal(
                    &runtime_dir,
                    &format!("owner evidence could not be loaded: {error}"),
                );
                continue;
            }
        };
        if bytes.len() > 4096 {
            say_reap_refusal(&runtime_dir, "owner evidence exceeds its size bound");
            continue;
        }
        let owner = match serde_json::from_slice::<SidecarOwner>(&bytes) {
            Ok(owner) => owner,
            Err(error) => {
                say_reap_refusal(&runtime_dir, &format!("owner evidence is invalid: {error}"));
                continue;
            }
        };
        let socket = runtime_dir.join(SOCKET_FILE);
        let endpoint = format!("unix://{}", socket.to_string_lossy());
        let mut matching = process_routes
            .remove(&endpoint)
            .map(|(_, processes)| processes)
            .unwrap_or_default();
        let creator_is_live = owner
            .creator_pid
            .zip(owner.creator_started.as_deref())
            .is_some_and(|(pid, started)| sample.matches_start(pid, started))
            || (owner.creator_pid.is_none()
                && owned_runtime_route(base, uid, &endpoint)
                    .is_some_and(|route| sample.contains_pid(route.creator_pid)));
        if creator_is_live && !matching.is_empty() {
            continue;
        }
        let owner_is_live = sample.matches_start(owner.pid, &owner.started);
        let owner_has_route = matching
            .iter()
            .any(|process| process.pid == owner.pid && process.started == owner.started);
        if let Some(process_group) = owner.process_group
            && (process_group != owner.pid
                || matching.iter().any(|process| {
                    process.pid == owner.pid && process.process_group != process_group
                }))
        {
            say_reap_refusal(
                &runtime_dir,
                "the recorded private process group no longer agrees with its leader",
            );
            continue;
        }
        if owner_is_live && !owner_has_route {
            say_reap_refusal(
                &runtime_dir,
                "the recorded pid is live but no longer has this exact app-server endpoint",
            );
            continue;
        }
        let stopped = stop_matching_sidecars(&endpoint, &mut matching);
        if stopped {
            if let Ok(socket_metadata) = std::fs::symlink_metadata(&socket)
                && (!socket_metadata.file_type().is_socket() || socket_metadata.uid() != uid)
            {
                say_reap_refusal(
                    &runtime_dir,
                    "the socket path is no longer an owned Unix socket",
                );
                continue;
            }
            cleanup_runtime_dir(&runtime_dir, &socket);
            reaped += 1;
        }
    }

    let mut missing_routes = process_routes.into_values().collect::<Vec<_>>();
    missing_routes.sort_by(|left, right| left.0.endpoint.cmp(&right.0.endpoint));
    for (route, mut processes) in missing_routes {
        if route.runtime_dir.exists() || sample.contains_pid(route.creator_pid) {
            continue;
        }
        if inspected >= STALE_REAP_MAX {
            eprintln!(
                "zerocode-shell: stale Codex reap stopped after its {STALE_REAP_MAX}-route startup bound"
            );
            break;
        }
        inspected += 1;
        if stop_matching_sidecars(&route.endpoint, &mut processes) {
            reaped += 1;
        }
    }
    reaped
}

#[cfg(not(unix))]
pub(crate) fn reap_stale() -> usize {
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The production table, under the names the assertions below read with.
    const PRODUCTION: LaunchBudget = LaunchBudget::PRODUCTION;
    const RETRY_DELAY: Duration = PRODUCTION.probe_retry_delay;

    /// The budget a test on a shared machine gets.
    ///
    /// The installed-CLI contract below does not get a machine to itself. It
    /// runs inside the whole suite, the release gate reaches it straight off
    /// a full webview compile, and the CLI it probes is reached through this
    /// window's own shim — so the probe waits on whatever the window is doing
    /// too. Measured here the probe answers in 0.10 s idle and 0.35 s under a
    /// load average of 44, and it still lost the production two-second bound
    /// in five release gates out of five: every one of them
    /// `CapabilityIndeterminate { stall: TimedOut, attempts: 1 }`, every one
    /// of them green on the solo rerun that followed.
    ///
    /// Twenty seconds is a hundredfold that idle answer, and a second try
    /// means one unlucky stall cannot decide a release; a genuinely wedged
    /// machine still gets its red inside forty seconds. The bound a loaded
    /// test machine needs is not the bound a person waiting on a worker
    /// launch should pay, so the patience lives here and
    /// [`LaunchBudget::PRODUCTION`] keeps its two seconds.
    const LOADED_MACHINE: LaunchBudget = LaunchBudget {
        probe_timeout: Duration::from_secs(20),
        probe_tries: 2,
        sidecar_ready: Duration::from_secs(20),
        ..PRODUCTION
    };

    #[cfg(unix)]
    fn write_test_owner(runtime_dir: &Path, pid: u32, started: &str) {
        use std::os::unix::fs::PermissionsExt as _;

        let bytes = serde_json::to_vec(&SidecarOwner {
            pid,
            started: started.to_string(),
            process_group: None,
            creator_pid: None,
            creator_started: None,
        })
        .expect("owner JSON");
        let path = runtime_dir.join(OWNER_FILE);
        std::fs::write(&path, bytes).expect("write owner fixture");
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .expect("private owner fixture");
    }

    #[cfg(unix)]
    fn spawn_fake_app_server(endpoint: &str) -> Child {
        crate::proc::quiet_command("/bin/sh")
            .args([
                "-c",
                "while :; do :; done",
                "app-server",
                "--listen",
                endpoint,
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("a fake app-server")
    }

    fn test_notice() -> PointerNotice {
        PointerNotice::new("run-1", "worker:w-2", "m-3", 2).expect("a pointer")
    }

    #[test]
    fn fresh_and_resume_argv_share_one_explicit_remote_prefix() {
        let endpoint = "unix:///tmp/zcx-test/app.sock";
        let mut fresh = vec!["--model".to_string(), "gpt-test".to_string()];
        let mut resumed = vec!["resume".to_string(), "thread-id".to_string()];
        prepend_remote(&mut fresh, endpoint);
        prepend_remote(&mut resumed, endpoint);

        assert_eq!(&fresh[..2], ["--remote", endpoint]);
        assert_eq!(&resumed[..2], ["--remote", endpoint]);
        assert_eq!(fresh.iter().filter(|arg| *arg == "--remote").count(), 1);
        assert_eq!(resumed.iter().filter(|arg| *arg == "--remote").count(), 1);
        assert!(has_remote(&fresh));
        assert!(has_remote(&["--remote=unix:///other".to_string()]));
    }

    #[test]
    fn queue_argv_contains_only_the_endpoint_thread_and_fixed_pointer() {
        let notice = test_notice();
        let args = queue_args("unix:///tmp/zcx/app.sock", "private-thread", notice.text());

        assert_eq!(
            args,
            [
                "queue",
                "--remote",
                "unix:///tmp/zcx/app.sock",
                "--thread",
                "private-thread",
                "--message",
                notice.text(),
            ]
        );
        assert!(!args.iter().any(|arg| arg.contains("message body")));
    }

    #[test]
    fn queue_environment_excludes_hook_team_and_account_credentials() {
        let env = vec![
            ("PATH".to_string(), "/bin".to_string()),
            (
                "ZEROCODE_LAUNCH_TOKEN".to_string(),
                "launch-secret".to_string(),
            ),
            ("OPENAI_API_KEY".to_string(), "account-secret".to_string()),
            ("CODEX_HOME".to_string(), "/managed/codex".to_string()),
            ("PATH".to_string(), "/new/bin".to_string()),
        ];

        assert_eq!(
            minimal_codex_env(&env),
            [
                ("CODEX_HOME".to_string(), "/managed/codex".to_string()),
                ("PATH".to_string(), "/new/bin".to_string()),
            ]
        );
    }

    #[test]
    fn queue_process_outcomes_preserve_the_unknown_boundary() {
        for refused in [CommandResult::ProgramMissing, CommandResult::SpawnFailed] {
            assert_eq!(
                classify_queue_result(refused),
                NotificationOutcome::DefinitelyUnsent
            );
        }
        assert_eq!(
            classify_queue_result(CommandResult::Succeeded),
            NotificationOutcome::Confirmed
        );
        for result in [
            CommandResult::Failed,
            CommandResult::TimedOut,
            CommandResult::WaitFailed,
        ] {
            assert_eq!(classify_queue_result(result), NotificationOutcome::Unknown);
        }
    }

    #[test]
    fn an_unbound_route_is_definitely_unsent_and_debug_is_redacted() {
        let notifier = CodexNotifier {
            program: PathBuf::from("/secret/program/codex"),
            cwd: PathBuf::from("/secret/worktree"),
            endpoint: "unix:///tmp/secret-route/app.sock".to_string(),
            socket: PathBuf::from("/tmp/secret-route/app.sock"),
            queue_env: vec![("CODEX_HOME".to_string(), "/secret/home".to_string())],
            thread_id: Mutex::new(None),
        };

        assert_eq!(
            notifier.notify(&test_notice()),
            NotificationOutcome::DefinitelyUnsent
        );
        let rendered = format!("{notifier:?} {:?}", CodexQueueError::SidecarSpawn);
        for secret in [
            "/secret/program/codex",
            "/secret/worktree",
            "secret-route",
            "/secret/home",
        ] {
            assert!(
                !rendered.contains(secret),
                "debug leaked {secret}: {rendered}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn runtime_socket_paths_fit_the_conservative_macos_boundary() {
        use std::os::unix::ffi::OsStrExt as _;

        let (runtime_dir, socket, endpoint) = create_runtime_dir().expect("a short runtime dir");
        assert!(socket.as_os_str().as_bytes().len() <= UNIX_SOCKET_PATH_MAX_BYTES);
        assert_eq!(endpoint, format!("unix://{}", socket.to_string_lossy()));
        let permissions = std::fs::symlink_metadata(&runtime_dir)
            .expect("runtime metadata")
            .permissions();
        use std::os::unix::fs::PermissionsExt as _;
        assert_eq!(permissions.mode() & 0o077, 0);
        cleanup_runtime_dir(&runtime_dir, &socket);
    }

    #[cfg(unix)]
    #[test]
    fn bounded_process_runner_distinguishes_success_failure_and_timeout() {
        let shell = Path::new("/bin/sh");
        assert_eq!(
            run_bounded_command(
                shell,
                &["-c".to_string(), "exit 0".to_string()],
                None,
                None,
                Duration::from_secs(1),
            ),
            CommandResult::Succeeded
        );
        assert_eq!(
            run_bounded_command(
                shell,
                &["-c".to_string(), "exit 7".to_string()],
                None,
                None,
                Duration::from_secs(1),
            ),
            CommandResult::Failed
        );
        assert_eq!(
            run_bounded_command(
                shell,
                &["-c".to_string(), "while :; do :; done".to_string()],
                None,
                None,
                Duration::from_millis(20),
            ),
            CommandResult::TimedOut
        );

        // A refusal to start is not one word. Which of the two it is decides
        // whether a later worker has anything new to measure.
        let root = tempfile::Builder::new()
            .prefix("zcx-spawn-")
            .tempdir_in("/tmp")
            .expect("a spawn fixture");
        let unreadable = root.path().join("not-executable");
        std::fs::write(&unreadable, b"#!/bin/sh\n").expect("the fixture file");
        for absent in [root.path().join("no-such-program"), unreadable] {
            assert_eq!(
                run_bounded_command(&absent, &[], None, None, Duration::from_secs(1)),
                CommandResult::ProgramMissing,
                "an unusable executable was reported as machine pressure"
            );
        }
        assert_eq!(
            spawn_result(std::io::ErrorKind::OutOfMemory),
            CommandResult::SpawnFailed
        );
    }

    #[cfg(unix)]
    fn probe_key(program: &str, env: &[(&str, &str)]) -> CapabilityKey {
        CapabilityKey {
            program: PathBuf::from(program),
            env: env
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect(),
        }
    }

    /// A fixture CLI that answers every probe, reachable ONLY through the
    /// launch PATH handed to it — never through this test process's own.
    #[cfg(unix)]
    fn fixture_cli(prefix: &str, command: &str) -> tempfile::TempDir {
        fixture_cli_running(prefix, command, "exit 0")
    }

    /// The same fixture with the shell body the probe will run.
    #[cfg(unix)]
    fn fixture_cli_running(prefix: &str, command: &str, body: &str) -> tempfile::TempDir {
        use std::os::unix::fs::PermissionsExt as _;

        let root = tempfile::Builder::new()
            .prefix(prefix)
            .tempdir_in("/tmp")
            .expect("a fixture CLI directory");
        let program = root.path().join(command);
        std::fs::write(&program, format!("#!/bin/sh\n{body}\n")).expect("the fixture CLI");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))
            .expect("the fixture CLI mode");
        assert!(
            !std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default())
                .any(|directory| directory == root.path()),
            "the fixture is already reachable without the launch environment"
        );
        root
    }

    /// The defect this module was carrying: the capability probe was the one
    /// Codex process started WITHOUT the worker's launch environment, so it
    /// measured whatever the window's own `PATH` could reach. A window opened
    /// from the dock has `/usr/bin:/bin:/usr/sbin:/sbin`, a summons says the
    /// bare word `codex`, and every worker on a machine with a working CLI
    /// fell to PTY while the retry re-measured the same unchanging PATH.
    #[cfg(unix)]
    #[test]
    fn a_capability_probe_measures_the_worker_launch_environment_not_the_window_path() {
        // A name no machine's own PATH can answer: if the probe still finds
        // something, it is because the launch environment led it there.
        let command = "zcx-probe-fixture-codex";
        let fixture = fixture_cli("zcx-cap-", command);
        let home = fixture.path().join("codex-home");
        let env = vec![
            (
                "PATH".to_string(),
                fixture.path().to_string_lossy().into_owned(),
            ),
            (
                "CODEX_HOME".to_string(),
                home.to_string_lossy().into_owned(),
            ),
        ];

        assert_eq!(
            capability(Path::new(command), &env, LOADED_MACHINE),
            CodexQueueCapability::Supported,
            "the probe answered for a program this worker was never going to run"
        );
        // Only the two values that decide which executable answers cross into
        // the probe; a launch secret must not ride along to say `--help`.
        let noisy = [
            env[0].clone(),
            ("ZEROCODE_LAUNCH_TOKEN".to_string(), "secret".to_string()),
            env[1].clone(),
        ];
        assert_eq!(minimal_codex_env(&noisy), [env[1].clone(), env[0].clone()]);
    }

    /// The two answers that come FROM the CLI are kept; the ones that come
    /// from a busy machine are not, and each retry waits longer than the last.
    #[cfg(unix)]
    #[test]
    fn a_transient_capability_stall_is_retried_with_a_widening_gap_instead_of_cached_forever() {
        let cache = Mutex::new(HashMap::new());
        let key = probe_key("/tmp/fake-codex-with-one-slow-start", &[("PATH", "/bin")]);
        let now = Instant::now();
        let mut calls = 0;

        assert_eq!(
            capability_with(&key, &cache, now, PRODUCTION, |_, _, _| {
                calls += 1;
                CommandResult::TimedOut
            }),
            CodexQueueCapability::Indeterminate {
                stall: ProbeStall::TimedOut,
                attempts: 1,
            }
        );
        assert_eq!(calls, 1);
        assert_eq!(
            capability_with(
                &key,
                &cache,
                now + RETRY_DELAY / 2,
                PRODUCTION,
                |_, _, _| {
                    calls += 1;
                    CommandResult::Succeeded
                },
            ),
            CodexQueueCapability::Indeterminate {
                stall: ProbeStall::TimedOut,
                attempts: 1,
            },
            "the cooldown did not suppress a probe storm during the same load burst"
        );
        assert_eq!(calls, 1);

        // A second silence counts, says which silence it was, and buys twice
        // the wait — a loop that is getting nowhere must stop reading like a
        // first failure.
        assert_eq!(
            capability_with(&key, &cache, now + RETRY_DELAY, PRODUCTION, |_, _, _| {
                calls += 1;
                CommandResult::SpawnFailed
            }),
            CodexQueueCapability::Indeterminate {
                stall: ProbeStall::Unspawnable,
                attempts: 2,
            }
        );
        assert_eq!(calls, 2);
        assert_eq!(
            capability_with(
                &key,
                &cache,
                now + RETRY_DELAY * 2,
                PRODUCTION,
                |_, _, _| {
                    calls += 1;
                    CommandResult::Succeeded
                },
            ),
            CodexQueueCapability::Indeterminate {
                stall: ProbeStall::Unspawnable,
                attempts: 2,
            },
            "the second silence did not widen the gap before the next measurement"
        );
        assert_eq!(calls, 2);
        assert_eq!(PRODUCTION.retry_after(1), RETRY_DELAY);
        assert_eq!(
            PRODUCTION.retry_after(u32::MAX),
            RETRY_DELAY * 2u32.pow(PRODUCTION.probe_retry_doublings),
            "the widening gap has no ceiling"
        );

        assert_eq!(
            capability_with(
                &key,
                &cache,
                now + RETRY_DELAY * 4,
                PRODUCTION,
                |_, _, _| {
                    calls += 1;
                    CommandResult::Succeeded
                },
            ),
            CodexQueueCapability::Supported,
            "a transient stall became a process-lifetime verdict"
        );
        assert_eq!(
            calls, 6,
            "the retry did not measure all four Codex surfaces"
        );
        assert_eq!(
            capability_with(
                &key,
                &cache,
                now + RETRY_DELAY * 8,
                PRODUCTION,
                |_, _, _| {
                    calls += 1;
                    CommandResult::Failed
                },
            ),
            CodexQueueCapability::Supported,
            "a successful measurement was not retained"
        );
        assert_eq!(calls, 6);
    }

    /// A CLI that is not there is a plain answer, not a stall: it is said
    /// once and not re-asked every fifteen seconds. It stays an answer about
    /// ONE launch environment, so the next worker's environment is measured on
    /// its own rather than inheriting a verdict it never earned.
    #[cfg(unix)]
    #[test]
    fn a_definite_verdict_is_kept_per_launch_environment_rather_than_re_probed() {
        let cache = Mutex::new(HashMap::new());
        let absent = probe_key("codex", &[("PATH", "/usr/bin:/bin")]);
        let installed = probe_key("codex", &[("PATH", "/opt/agents/bin:/usr/bin:/bin")]);
        let now = Instant::now();
        let mut calls = 0;

        for (round, at) in [now, now + RETRY_DELAY * 100].into_iter().enumerate() {
            assert_eq!(
                capability_with(&absent, &cache, at, PRODUCTION, |_, _, _| {
                    calls += 1;
                    CommandResult::ProgramMissing
                }),
                CodexQueueCapability::Missing
            );
            assert_eq!(
                calls, 1,
                "round {round} re-asked a question already answered"
            );
        }

        assert_eq!(
            capability_with(&installed, &cache, now, PRODUCTION, |_, _, _| {
                calls += 1;
                CommandResult::Succeeded
            }),
            CodexQueueCapability::Supported,
            "one launch environment's missing CLI answered for another's"
        );
        assert_eq!(calls, 5);
    }

    /// Every way the route can be refused says which way it was. "Could not
    /// finish" over a spawn refusal, a timeout and a broken wait is what cost
    /// a day of hunting through a CLI that was answering the whole time.
    #[test]
    fn every_route_refusal_names_which_failure_it_was() {
        let spoken = [
            CodexQueueError::UnsupportedCli.to_string(),
            CodexQueueError::CliMissing.to_string(),
            CodexQueueError::CapabilityIndeterminate {
                stall: ProbeStall::Unspawnable,
                attempts: 1,
            }
            .to_string(),
            CodexQueueError::CapabilityIndeterminate {
                stall: ProbeStall::TimedOut,
                attempts: 1,
            }
            .to_string(),
            CodexQueueError::CapabilityIndeterminate {
                stall: ProbeStall::Unwaitable,
                attempts: 1,
            }
            .to_string(),
        ];
        let distinct: std::collections::HashSet<&String> = spoken.iter().collect();
        assert_eq!(
            distinct.len(),
            spoken.len(),
            "two refusals read alike: {spoken:?}"
        );

        let repeated = CodexQueueError::CapabilityIndeterminate {
            stall: ProbeStall::TimedOut,
            attempts: 7,
        }
        .to_string();
        assert!(
            repeated.contains("attempt 7"),
            "a retry that is getting nowhere reads like its first try: {repeated}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn bound_queue_process_receives_the_fixed_pointer_and_minimal_environment() {
        use std::os::unix::fs::PermissionsExt as _;
        use std::os::unix::net::UnixListener;

        let root = tempfile::Builder::new()
            .prefix("zcx-queue-")
            .tempdir_in("/tmp")
            .expect("a short queue fixture");
        let program = root.path().join("fake-codex");
        std::fs::write(
            &program,
            b"#!/bin/sh\n/usr/bin/env > queue.env\nprintf '%s\\0' \"$@\" > queue.args\n",
        )
        .expect("the fake Codex executable");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))
            .expect("the fake executable mode");
        let socket = root.path().join("app.sock");
        let _listener = UnixListener::bind(&socket).expect("a live route socket");
        let endpoint = format!("unix://{}", socket.to_string_lossy());
        let notifier = CodexNotifier {
            program,
            cwd: root.path().to_path_buf(),
            endpoint: endpoint.clone(),
            socket,
            queue_env: vec![
                ("CODEX_HOME".to_string(), "/private/codex-home".to_string()),
                ("PATH".to_string(), "/usr/bin:/bin".to_string()),
            ],
            thread_id: Mutex::new(Some("private-thread-id".to_string())),
        };
        let notice = test_notice();

        assert_eq!(notifier.notify(&notice), NotificationOutcome::Confirmed);
        let args = std::fs::read(root.path().join("queue.args")).expect("captured queue argv");
        let args: Vec<&str> = args
            .split(|byte| *byte == 0)
            .filter(|arg| !arg.is_empty())
            .map(|arg| std::str::from_utf8(arg).expect("UTF-8 argv"))
            .collect();
        assert_eq!(
            args,
            queue_args(&endpoint, "private-thread-id", notice.text())
        );
        let env = std::fs::read_to_string(root.path().join("queue.env"))
            .expect("captured queue environment");
        assert!(env.contains("CODEX_HOME=/private/codex-home"));
        assert!(env.contains("PATH=/usr/bin:/bin"));
        assert!(!env.contains("ZEROCODE_") && !env.contains("OPENAI_API_KEY"));
    }

    #[cfg(unix)]
    #[test]
    fn stale_reap_cleans_a_dead_owned_route_without_touching_other_paths() {
        let base = tempfile::Builder::new()
            .prefix("zcx-reap-")
            .tempdir_in("/tmp")
            .expect("a short test base");
        let (runtime_dir, socket, _) =
            create_runtime_dir_in(base.path()).expect("an owned runtime dir");
        let bystander = base.path().join("keep.txt");
        std::fs::write(&bystander, b"keep").expect("a bystander");
        write_test_owner(&runtime_dir, u32::MAX, "no-such-process");

        assert_eq!(reap_stale_in(base.path()), 1);
        assert!(!runtime_dir.exists());
        assert_eq!(std::fs::read(&bystander).expect("the bystander"), b"keep");
        assert!(!socket.exists());
    }

    #[cfg(unix)]
    #[test]
    fn stale_reap_refuses_a_runtime_directory_with_public_permissions() {
        use std::os::unix::fs::PermissionsExt as _;

        let base = tempfile::Builder::new()
            .prefix("zcx-hostile-")
            .tempdir_in("/tmp")
            .expect("a short test base");
        let (runtime_dir, socket, endpoint) =
            create_runtime_dir_in(base.path()).expect("a runtime dir");
        let mut child = spawn_fake_app_server(&endpoint);
        let started = crate::resource_usage::process_start_identity(child.id())
            .expect("the bystander identity");
        assert_eq!(
            crate::resource_usage::process_has_args(
                child.id(),
                &["app-server", "--listen", &endpoint]
            ),
            Ok(true),
            "the fixture does not exercise argv validation"
        );
        write_test_owner(&runtime_dir, child.id(), &started);
        std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o755))
            .expect("make the fixture untrusted");

        assert_eq!(reap_stale_in(base.path()), 0);
        assert!(child.try_wait().expect("sample bystander").is_none());
        assert!(runtime_dir.exists());

        let _ = child.kill();
        let _ = child.wait();
        std::fs::set_permissions(&runtime_dir, std::fs::Permissions::from_mode(0o700))
            .expect("restore private cleanup permissions");
        cleanup_runtime_dir(&runtime_dir, &socket);
    }

    #[cfg(unix)]
    #[test]
    fn stale_reap_finds_an_owned_sidecar_after_its_runtime_directory_is_gone() {
        use std::os::unix::fs::DirBuilderExt as _;

        let base = tempfile::Builder::new()
            .prefix("zcx-missing-route-")
            .tempdir_in("/tmp")
            .expect("a short test base");
        let uid = unsafe { libc::getuid() };
        let runtime_dir = base.path().join(format!(
            "{RUNTIME_DIR_PREFIX}{uid}-{}-{}",
            u32::MAX,
            u64::MAX
        ));
        let mut builder = std::fs::DirBuilder::new();
        builder.mode(0o700);
        builder.create(&runtime_dir).expect("a runtime directory");
        let socket = runtime_dir.join(SOCKET_FILE);
        let endpoint = format!("unix://{}", socket.to_string_lossy());
        let child = spawn_fake_app_server(&endpoint);
        std::fs::remove_dir(&runtime_dir).expect("the runtime directory disappears first");
        let watcher = std::thread::spawn(move || {
            let mut child = child;
            let deadline = Instant::now() + Duration::from_secs(6);
            loop {
                if child.try_wait().expect("sample fake sidecar").is_some() {
                    return true;
                }
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return false;
                }
                std::thread::sleep(PROCESS_POLL_INTERVAL);
            }
        });

        let reaped = reap_stale_in(base.path());
        let stopped = watcher.join().expect("the fake sidecar watcher");

        assert_eq!(reaped, 1, "a directory-only walk cannot see this route");
        assert!(
            stopped,
            "the sidecar survived its missing runtime directory"
        );
    }

    #[cfg(unix)]
    #[test]
    fn stale_reap_leaves_a_missing_directory_route_with_a_live_creator_alone() {
        let base = tempfile::Builder::new()
            .prefix("zcx-live-route-")
            .tempdir_in("/tmp")
            .expect("a short test base");
        let (runtime_dir, _, endpoint) =
            create_runtime_dir_in(base.path()).expect("a runtime directory");
        let mut child = spawn_fake_app_server(&endpoint);
        std::fs::remove_dir(&runtime_dir).expect("the runtime directory disappears first");

        assert_eq!(reap_stale_in(base.path()), 0);
        assert!(
            child.try_wait().expect("the live fake sidecar").is_none(),
            "reap killed a route whose creator is still alive"
        );

        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(unix)]
    #[test]
    fn dropping_a_sidecar_stops_the_wrapper_and_its_descendant() {
        let base = tempfile::Builder::new()
            .prefix("zcx-group-")
            .tempdir_in("/tmp")
            .expect("a short test base");
        let (runtime_dir, socket, _) =
            create_runtime_dir_in(base.path()).expect("a runtime directory");
        let child_pid_path = base.path().join("child.pid");
        let mut command = crate::proc::quiet_command("/bin/sh");
        command
            .args([
                "-c",
                "/usr/bin/tail -f /dev/null & echo $! > \"$1\"; wait",
                "sidecar-wrapper",
                child_pid_path
                    .to_str()
                    .expect("the temporary pid path is UTF-8"),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        prepare_process_group(&mut command);
        let child = command.spawn().expect("a fake wrapper");
        let wrapper_pid = child.id();
        let wrapper_started = crate::resource_usage::process_start_identity(wrapper_pid)
            .expect("the wrapper identity");
        let deadline = Instant::now() + Duration::from_secs(2);
        let descendant_pid = loop {
            if let Ok(pid) = std::fs::read_to_string(&child_pid_path)
                && let Ok(pid) = pid.trim().parse::<u32>()
            {
                break pid;
            }
            assert!(
                Instant::now() < deadline,
                "the fake wrapper did not report its descendant"
            );
            std::thread::sleep(PROCESS_POLL_INTERVAL);
        };
        let descendant_started = crate::resource_usage::process_start_identity(descendant_pid)
            .expect("the descendant identity");
        // SAFETY: both positive pids have just been sampled as the exact live
        // fixture processes; `getpgid` does not dereference pointers.
        let wrapper_group = unsafe { libc::getpgid(wrapper_pid as libc::pid_t) };
        let descendant_group = unsafe { libc::getpgid(descendant_pid as libc::pid_t) };
        assert_eq!(wrapper_group, wrapper_pid as libc::pid_t);
        assert_eq!(descendant_group, wrapper_group);
        let creator_started = crate::resource_usage::process_start_identity(std::process::id())
            .expect("the test runner identity");
        let sidecar = CodexSidecar {
            child,
            started: wrapper_started.clone(),
            creator_started,
            process_group: wrapper_pid,
            runtime_dir: runtime_dir.clone(),
            socket,
        };

        drop(sidecar);

        assert_ne!(
            crate::resource_usage::process_start_identity(wrapper_pid).as_deref(),
            Ok(wrapper_started.as_str()),
            "the wrapper survived its sidecar owner"
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        while crate::resource_usage::process_start_identity(descendant_pid).as_deref()
            == Ok(descendant_started.as_str())
            && Instant::now() < deadline
        {
            std::thread::sleep(PROCESS_POLL_INTERVAL);
        }
        assert_ne!(
            crate::resource_usage::process_start_identity(descendant_pid).as_deref(),
            Ok(descendant_started.as_str()),
            "the wrapper's descendant survived its sidecar owner"
        );
        assert!(!runtime_dir.exists(), "the runtime directory survived");
    }

    /// The installed CLI, driven exactly the way a summons drives it: a BARE
    /// command name that only the worker's launch `PATH` can resolve.
    ///
    /// The earlier shape of this test handed `prepare` an absolute path it had
    /// resolved itself, against a launch `PATH` copied from the test runner —
    /// so the probe's inherited environment found the binary anyway and the
    /// window's real failure could not appear here. The name below exists on
    /// no machine's `PATH`; only the launch environment leads to it.
    #[cfg(unix)]
    /// The guard behind the shims refuses a piped run from an orchestration
    /// seat, which is what a sidecar looks like from outside. Measured through
    /// this window's own shim: exit 2 in 0.03s, against 0.12s to a live socket
    /// on the real binary. So the queue resolves past them — but only when a
    /// shim is genuinely what would have run.
    #[test]
    fn a_sidecar_resolves_past_this_window_s_own_shims() {
        use std::os::unix::fs::PermissionsExt as _;

        let home = tempfile::Builder::new()
            .prefix("zcx-shim-walk-")
            .tempdir_in("/tmp")
            .expect("a directory to lay a PATH in");
        let shims = home.path().join("zerocode-shims-1");
        let real = home.path().join("bin");
        for dir in [&shims, &real] {
            std::fs::create_dir(dir).expect("a PATH directory");
            let program = dir.join("codex");
            std::fs::write(&program, "#!/bin/sh\nexit 0\n").expect("an executable");
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
                .expect("the executable bit");
        }
        let with_shims =
            std::env::join_paths([shims.as_path(), real.as_path()]).expect("a joined PATH");
        let bare = Path::new("codex");

        assert_eq!(
            past_the_shims_in(bare, Some(&with_shims), Some(&shims)),
            real.join("codex"),
            "the shim at the head of the PATH is walked past"
        );

        // Everything else is left alone, because rewriting it would be a
        // guess rather than a repair.
        assert_eq!(
            past_the_shims_in(bare, Some(&with_shims), None),
            bare,
            "a window that installed no shims changes nothing"
        );
        let only_real = std::env::join_paths([real.as_path()]).expect("a joined PATH");
        assert_eq!(
            past_the_shims_in(bare, Some(&only_real), Some(&shims)),
            bare,
            "a PATH the shim is not on is already resolving the real binary"
        );
        let absolute = shims.join("codex");
        assert_eq!(
            past_the_shims_in(&absolute, Some(&with_shims), Some(&shims)),
            absolute,
            "an absolute program never consults PATH, so it is not second-guessed"
        );
        assert_eq!(
            past_the_shims_in(bare, None, Some(&shims)),
            bare,
            "a launch environment carrying no PATH is left as it is"
        );

        // A shim with nothing behind it stays the answer: a sidecar that
        // cannot start is a better outcome than one started from a guess.
        let lonely = home.path().join("zerocode-shims-2");
        std::fs::create_dir(&lonely).expect("a second shim directory");
        let program = lonely.join("codex");
        std::fs::write(&program, "#!/bin/sh\nexit 0\n").expect("an executable");
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755))
            .expect("the executable bit");
        let alone = std::env::join_paths([lonely.as_path()]).expect("a joined PATH");
        assert_eq!(
            past_the_shims_in(bare, Some(&alone), Some(&lonely)),
            bare,
            "a shim with no real binary behind it is left in place"
        );
    }

    /// The release gate's red, on demand.
    ///
    /// Five release gates in a row refused the installed CLI with
    /// `CapabilityIndeterminate { stall: TimedOut, attempts: 1 }` and passed
    /// on every solo rerun, which is the shape of a bound losing to load
    /// rather than a CLI missing a surface. Shrinking the bound to a
    /// millisecond reproduces that exact refusal on any machine, loaded or
    /// idle — so what the contract below proves is the widened budget, not
    /// the weather on the day it ran.
    #[cfg(unix)]
    #[test]
    fn a_probe_bound_too_small_to_answer_inside_is_the_release_gate_s_refusal() {
        let command = "zcx-starved-fixture-codex";
        // Absolute: the probe hands its child a `PATH` with nothing on it but
        // the fixture, so a bare `sleep` would exit 127 in a millisecond or
        // two and answer "unsupported" instead of never answering at all.
        let fixture = fixture_cli_running("zcx-starved-", command, "/bin/sleep 0.5");
        let env = vec![(
            "PATH".to_string(),
            fixture.path().to_string_lossy().into_owned(),
        )];
        let mut args = vec!["--dangerously-bypass-approvals-and-sandbox".to_string()];
        let starved = LaunchBudget {
            probe_timeout: Duration::from_millis(1),
            ..PRODUCTION
        };

        assert_eq!(
            prepare_with(
                Path::new(command),
                &mut args,
                Path::new("/tmp"),
                &env,
                starved,
            )
            .err(),
            Some(CodexQueueError::CapabilityIndeterminate {
                stall: ProbeStall::TimedOut,
                attempts: 1,
            }),
            "the probe bound is not the parameter the refusal came from"
        );
        assert_eq!(
            args,
            ["--dangerously-bypass-approvals-and-sandbox"],
            "a refused route still rewrote the launch argv"
        );
    }

    /// How many times one measurement asks is a bound too, and only silence
    /// is worth re-asking: a machine that answered has already been heard.
    #[cfg(unix)]
    #[test]
    fn a_measurement_re_asks_only_silence_and_only_as_often_as_its_budget() {
        let patient = LaunchBudget {
            probe_tries: 2,
            ..PRODUCTION
        };

        let cache = Mutex::new(HashMap::new());
        let key = probe_key("/tmp/fake-codex-slow-once", &[("PATH", "/bin")]);
        let mut calls = 0;
        assert_eq!(
            capability_with(&key, &cache, Instant::now(), patient, |_, _, _| {
                calls += 1;
                if calls == 1 {
                    CommandResult::TimedOut
                } else {
                    CommandResult::Succeeded
                }
            }),
            CodexQueueCapability::Supported,
            "one stall inside the try count was allowed to stand as no answer"
        );
        assert_eq!(calls, 5, "four surfaces, the first of them asked twice");

        let cache = Mutex::new(HashMap::new());
        let key = probe_key("/tmp/fake-codex-always-slow", &[("PATH", "/bin")]);
        let mut calls = 0;
        assert_eq!(
            capability_with(&key, &cache, Instant::now(), patient, |_, _, _| {
                calls += 1;
                CommandResult::TimedOut
            }),
            CodexQueueCapability::Indeterminate {
                stall: ProbeStall::TimedOut,
                attempts: 1,
            }
        );
        assert_eq!(calls, 2, "the try count is a bound, not a loop");

        let cache = Mutex::new(HashMap::new());
        let key = probe_key("/tmp/fake-codex-without-the-surface", &[("PATH", "/bin")]);
        let mut calls = 0;
        assert_eq!(
            capability_with(&key, &cache, Instant::now(), patient, |_, _, _| {
                calls += 1;
                CommandResult::Failed
            }),
            CodexQueueCapability::Unsupported
        );
        assert_eq!(calls, 1, "a verdict from the CLI was second-guessed");

        // The one a worker launch runs under asks once, so a person waiting
        // on a summons never pays for a machine that is not answering.
        assert_eq!(PRODUCTION.probe_tries, 1);
        assert_eq!(PRODUCTION.probe_timeout, Duration::from_secs(2));
        assert_eq!(PRODUCTION.sidecar_ready, Duration::from_secs(5));
    }

    #[cfg(unix)]
    #[test]
    fn installed_codex_starts_and_cleans_an_explicit_worker_sidecar() {
        use std::os::unix::fs::PermissionsExt as _;

        let runnable = |candidate: &Path| {
            std::fs::metadata(candidate)
                .is_ok_and(|held| held.is_file() && held.permissions().mode() & 0o111 != 0)
        };
        let overridden = std::env::var_os("ZEROCODE_CODEX_BIN").map(PathBuf::from);
        // A machine without Codex skips; a machine that NAMES one must run.
        if let Some(named) = overridden.as_deref() {
            assert!(
                runnable(named),
                "ZEROCODE_CODEX_BIN does not name a runnable Codex executable"
            );
        }
        let program = overridden.or_else(|| {
            std::env::var_os("PATH").and_then(|path| {
                std::env::split_paths(&path)
                    .map(|directory| directory.join("codex"))
                    .find(|candidate| runnable(candidate))
            })
        });
        let Some(program) = program else {
            eprintln!("skipped installed Codex contract: codex is not installed on PATH");
            return;
        };

        let home = tempfile::Builder::new()
            .prefix("zcx-installed-")
            .tempdir_in("/tmp")
            .expect("an isolated Codex home");
        // The summons word: unique, so nothing but the launch PATH can find
        // it, and the machine's own `node` stays reachable behind it.
        let command = "zcx-codex-under-test";
        let reachable = home.path().join("bin");
        std::fs::create_dir(&reachable).expect("the launch bin directory");
        std::os::unix::fs::symlink(&program, reachable.join(command))
            .expect("the launch-only name for the installed CLI");
        let launch_path = format!(
            "{}:{}",
            reachable.to_string_lossy(),
            std::env::var("PATH").expect("a launch PATH")
        );
        let mut args = vec!["--dangerously-bypass-approvals-and-sandbox".to_string()];
        let env = vec![
            (
                "CODEX_HOME".to_string(),
                home.path().to_string_lossy().into_owned(),
            ),
            ("PATH".to_string(), launch_path),
        ];

        let pending = prepare_with(
            Path::new(command),
            &mut args,
            Path::new("/tmp"),
            &env,
            LOADED_MACHINE,
        )
        .expect("the installed CLI sidecar");
        let runtime_dir = pending
            .sidecar
            .as_ref()
            .expect("the pending sidecar")
            .runtime_dir
            .clone();
        assert_eq!(args.first().map(String::as_str), Some("--remote"));
        assert!(runtime_dir.join(SOCKET_FILE).exists());

        drop(pending);
        assert!(
            !runtime_dir.exists(),
            "the sidecar runtime directory survived"
        );
    }

    /// t-7812 B: a resumed thread keeps the PTY road. The ledger's reseat of
    /// a Codex worker on 2026-09-25 (w-7738, 01:03:26) put the app-server
    /// remote in front of `resume` beside the launch's permission override,
    /// and Codex answered "Error: Permission overrides are not supported when
    /// resuming a remote task." and exited 1.7 s in — the worker's death, and
    /// the empty Codex the next door opened in its place. A resume argv is
    /// left exactly as it was, and no sidecar is started for it.
    #[cfg(unix)]
    #[test]
    fn a_resumed_codex_thread_keeps_its_argv_and_starts_no_sidecar() {
        use std::os::unix::fs::PermissionsExt as _;

        let runnable = |candidate: &Path| {
            std::fs::metadata(candidate)
                .is_ok_and(|held| held.is_file() && held.permissions().mode() & 0o111 != 0)
        };
        let program = std::env::var_os("ZEROCODE_CODEX_BIN")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("PATH").and_then(|path| {
                    std::env::split_paths(&path)
                        .map(|directory| directory.join("codex"))
                        .find(|candidate| runnable(candidate))
                })
            });
        let Some(program) = program else {
            eprintln!("skipped the resumed-thread contract: codex is not installed on PATH");
            return;
        };
        let home = tempfile::Builder::new()
            .prefix("zcx-resume-")
            .tempdir_in("/tmp")
            .expect("an isolated Codex home");
        let command = "zcx-codex-resuming";
        let reachable = home.path().join("bin");
        std::fs::create_dir(&reachable).expect("the launch bin directory");
        std::os::unix::fs::symlink(&program, reachable.join(command))
            .expect("the launch-only name for the installed CLI");
        let env = vec![
            (
                "CODEX_HOME".to_string(),
                home.path().to_string_lossy().into_owned(),
            ),
            (
                "PATH".to_string(),
                format!(
                    "{}:{}",
                    reachable.to_string_lossy(),
                    std::env::var("PATH").expect("a launch PATH")
                ),
            ),
        ];
        // The reseat's own line: the launch plan's override, the thread.
        let mut args = vec![
            "--dangerously-bypass-approvals-and-sandbox".to_string(),
            "resume".to_string(),
            "01a0d420-5b6c-7621-9f74-c792f7dc2d36".to_string(),
        ];
        let before = args.clone();
        let pending = prepare_with(
            Path::new(command),
            &mut args,
            Path::new("/tmp"),
            &env,
            LOADED_MACHINE,
        );
        assert_eq!(
            args, before,
            "a resume line was rewritten onto the remote route Codex refuses"
        );
        assert!(
            pending.is_err(),
            "a sidecar was started for a resumed thread"
        );
    }
}
