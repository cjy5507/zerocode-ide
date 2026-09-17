//! Session-persistent Python kernel behind the `REPL` tool.
//!
//! The one-shot REPL path (`python3 -c <code>`) forgets everything between
//! calls, so every follow-up pays to re-read and re-parse state the model
//! already computed. This module keeps ONE Python process per (session,
//! language) alive across tool calls: variables, imports, functions, and
//! parsed data survive turns — and context compaction, since the state lives
//! in the child process rather than in the transcript.
//!
//! Design constraints, in order:
//! - **No runtime dependencies.** The bootstrap is stdlib-only Python fed via
//!   `-c`; there is no ipykernel/Jupyter/venv to provision, so the kernel
//!   works on any machine where `python3` resolves.
//! - **An exception is a result, not a crash.** A raised exception returns
//!   `ok:false` with the traceback while the kernel (and its namespace)
//!   stays alive — matching interactive REPL semantics.
//! - **A dead kernel is disposable.** Timeout or EOF kills and unregisters
//!   the kernel; the next call spawns a fresh one. State loss is reported in
//!   the error text so the model knows to rebuild.
//! - **Orphan-free.** The bootstrap's read loop ends on stdin EOF, so kernels
//!   exit on their own when the zo process goes away.
//!
//! Protocol: one JSON object per line on stdin (`{"id","code"}`), one JSON
//! object per line on stdout (`{"id","ok","value","stdout","stderr","error"}`).
//! stdout/stderr of the executed code are captured and never interleave with
//! protocol frames.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde::Deserialize;
use serde_json::json;

use super::error::ToolError;

/// Default per-exec deadline. A persistent kernel read with no deadline would
/// hang the tool forever on an infinite loop, so unlike the one-shot path the
/// persistent path always has one; mirrors the `bash` tool's default.
const DEFAULT_EXEC_TIMEOUT: Duration = Duration::from_millis(120_000);

/// Byte caps applied on the Rust side before the result enters the transcript.
/// Generous enough for real data inspection, bounded enough that one stray
/// `print(huge)` cannot flood a turn. The Python side additionally caps the
/// repr it produces (`MAX_VALUE_REPR_CHARS` in the bootstrap).
const MAX_STREAM_BYTES: usize = 200_000;

/// The same budget, applied by the dispatch bridge to an agent report handed
/// back through `zo.result` — defined AS the stream cap so the two can never
/// drift apart.
pub(crate) const BRIDGE_REPORT_MAX_BYTES: usize = MAX_STREAM_BYTES;

/// Spawn-to-first-frame deadline. Interpreter startup is tens of
/// milliseconds; seconds mean a broken install and should fail loudly.
const SPAWN_PROBE_TIMEOUT: Duration = Duration::from_millis(10_000);

/// Most kernels one zo process keeps alive at once. Sessions are serial in
/// practice, but `/clear` mints a new session id while the old session's
/// kernel would otherwise idle forever — beyond this bound the
/// longest-idle kernel is killed when a new one is admitted.
const MAX_KERNELS: usize = 8;

/// Stdlib-only bootstrap. Executes each request in a single shared namespace
/// and evaluates a trailing expression like a REPL. `__name__` is set so
/// `if __name__ == "__main__"` blocks behave as in a script.
///
/// The `zo` object is the sub-agent bridge: `zo.spawn(...)` / `zo.result(...)`
/// write a bridge frame to the REAL stdout (`sys.__stdout__` — during exec the
/// visible `sys.stdout` is redirected into the cell's captured stream) and
/// block on ONE stdin line for the host's reply. The main loop uses
/// `readline()` rather than iterating `sys.stdin` because the iterator's
/// readahead could swallow a bridge reply into its private buffer and
/// deadlock the cell against the host.
const PYTHON_BOOTSTRAP: &str = r#"
import ast, contextlib, io, json, os, pickle, sys, traceback

MAX_VALUE_REPR_CHARS = 10_000

# Namespace snapshot: the host hands a per-scope path via ZO_KERNEL_SNAPSHOT
# (popped so user code and its subprocesses never see it). After every cell
# the picklable subset of the namespace is dumped atomically; a fresh kernel
# for the same scope restores it, so a timeout, eviction, or zo restart
# loses the process but not the data. Everything here is best-effort: a
# snapshot failure must never break the cell that triggered it.
_SNAPSHOT_PATH = os.environ.pop("ZO_KERNEL_SNAPSHOT", None)
_SNAPSHOT_VALUE_CAP = 2_000_000
_SNAPSHOT_TOTAL_CAP = 8_000_000
_SNAPSHOT_RESERVED = ("__name__", "zo")

def _snapshot_save(ns):
    if not _SNAPSHOT_PATH:
        return
    try:
        blobs, total = {}, 0
        for key in sorted(ns):
            if key in _SNAPSHOT_RESERVED or key.startswith("_"):
                continue
            try:
                blob = pickle.dumps(ns[key], protocol=pickle.HIGHEST_PROTOCOL)
            except Exception:
                continue  # unpicklable values (open files, modules, …) just don't survive
            if len(blob) > _SNAPSHOT_VALUE_CAP or total + len(blob) > _SNAPSHOT_TOTAL_CAP:
                continue
            blobs[key] = blob
            total += len(blob)
        tmp = _SNAPSHOT_PATH + ".tmp"
        with open(tmp, "wb") as f:
            pickle.dump({"v": 1, "vars": blobs}, f, protocol=pickle.HIGHEST_PROTOCOL)
        os.replace(tmp, _SNAPSHOT_PATH)
    except Exception:
        pass

def _snapshot_restore(ns):
    names = []
    if not _SNAPSHOT_PATH or not os.path.exists(_SNAPSHOT_PATH):
        return names
    try:
        with open(_SNAPSHOT_PATH, "rb") as f:
            snap = pickle.load(f)
        for key, blob in sorted((snap.get("vars") or {}).items()):
            if key in _SNAPSHOT_RESERVED:
                continue
            try:
                ns[key] = pickle.loads(blob)
                names.append(key)
            except Exception:
                pass  # a value whose class vanished only loses that one key
    except Exception:
        return []
    return names

class _ZoBridge:
    """Talk to the zo host mid-cell. Each call is one request frame out on the
    real stdout and one reply frame in from stdin; the host services it with
    the SAME tool machinery (permissions, routing, spawn caps) a direct tool
    call gets."""
    def __init__(self):
        self._next = 0
    def _call(self, op, args):
        self._next += 1
        rid = self._next
        sys.__stdout__.write(json.dumps({"zo_bridge": rid, "op": op, "args": args}) + "\n")
        sys.__stdout__.flush()
        while True:
            line = sys.stdin.readline()
            if not line:
                raise RuntimeError("zo bridge: host closed the channel")
            try:
                frame = json.loads(line)
            except Exception:
                continue
            if frame.get("zo_bridge") != rid:
                continue
            payload = frame.get("payload") or {}
            if payload.get("error"):
                raise RuntimeError(payload["error"])
            return payload.get("result")
    def spawn(self, prompt, description=None, agent_type=None, model=None):
        """Launch a background sub-agent; returns at spawn with its handle
        (a dict carrying agentId). Collect the report later with
        zo.result(agent_id) — do not busy-poll in a single cell."""
        if description is None:
            description = prompt if len(prompt) <= 60 else prompt[:57] + "..."
        return self._call("spawn", {
            "prompt": prompt,
            "description": description,
            "subagent_type": agent_type,
            "model": model,
        })
    def result(self, agent_id):
        """Current status of a spawned agent, plus its report text once the
        agent has written one. Non-blocking: while the agent runs you get
        status without a report."""
        return self._call("result", {"agent_id": agent_id})
    def skill(self, name):
        """An approved skill as data: its instruction text plus the directory
        holding its assets, so kernel code can read templates or
        sys.path.insert(0, info["dir"]) and import shipped helpers. Proposed
        (unreviewed) skills stay blocked, same as the Skill tool."""
        return self._call("skill", {"name": name})
    def history(self, contains=None, role=None, limit=20):
        """Search this session's own transcript as data: the most recent
        `limit` messages (chronological order) whose text contains
        `contains` (case-insensitive), optionally filtered to one role
        ("user"/"assistant"/"tool"/"system"). Each hit is a dict with role
        and text — assign it to a variable and slice/filter in code instead
        of re-reading the conversation."""
        return self._call("history", {"contains": contains, "role": role, "limit": limit})

ns = {"__name__": "__main__", "zo": _ZoBridge()}
# Restored names ride on the FIRST response frame only (the host's readiness
# probe), where the host records them; repeating them on every frame would
# tax each cell for a boot-time fact.
_restored_pending = _snapshot_restore(ns)

while True:
    line = sys.stdin.readline()
    if not line:
        break
    line = line.strip()
    if not line:
        continue
    try:
        req = json.loads(line)
    except Exception:
        continue
    out, err = io.StringIO(), io.StringIO()
    resp = {"id": req.get("id"), "ok": True, "value": None, "error": None}
    if _restored_pending is not None:
        resp["restored"] = _restored_pending
        _restored_pending = None
    try:
        with contextlib.redirect_stdout(out), contextlib.redirect_stderr(err):
            tree = ast.parse(req.get("code", ""), "<zo-repl>", "exec")
            value = None
            if tree.body and isinstance(tree.body[-1], ast.Expr):
                last = ast.Expression(tree.body.pop(-1).value)
                if tree.body:
                    exec(compile(tree, "<zo-repl>", "exec"), ns)
                value = eval(compile(last, "<zo-repl>", "eval"), ns)
            else:
                exec(compile(tree, "<zo-repl>", "exec"), ns)
            if value is not None:
                rendered = repr(value)
                if len(rendered) > MAX_VALUE_REPR_CHARS:
                    rendered = rendered[:MAX_VALUE_REPR_CHARS] + "…(truncated)"
                resp["value"] = rendered
    except BaseException:
        resp["ok"] = False
        resp["error"] = traceback.format_exc()
    resp["stdout"] = out.getvalue()
    resp["stderr"] = err.getvalue()
    sys.stdout.write(json.dumps(resp) + "\n")
    sys.stdout.flush()
    # After the reply, not before: the host is already reading the result
    # while the dump runs, so durability costs no cell latency. Worst case a
    # crash right here loses one cell's delta — and the restore report names
    # exactly what survived, so nothing is silently misrepresented.
    _snapshot_save(ns)
"#;

/// One live kernel: the child process, its stdin, and the reader-thread
/// channel its stdout frames arrive on. Held behind a per-kernel mutex so two
/// concurrent tool calls against the same session serialize (one namespace,
/// one cell at a time — the same rule Jupyter enforces) without blocking
/// kernels of other sessions.
struct KernelHandle {
    child: Child,
    stdin: ChildStdin,
    frames: Receiver<String>,
    /// Monotonic request counter; responses are matched on it so a stale
    /// frame from a timed-out call can never satisfy a later call.
    next_request: u64,
    started: Instant,
    /// Last successful exec — the LRU axis for [`MAX_KERNELS`] eviction.
    last_used: Instant,
    /// Variable names this kernel restored from its scope's snapshot at
    /// boot, reported once on the readiness probe's frame. Empty when there
    /// was nothing (or no snapshot path).
    restored_vars: Vec<String>,
}

/// Result of one persistent execution, shaped by the caller into the tool's
/// JSON reply.
#[derive(Debug)]
pub(crate) struct KernelExecOutput {
    pub ok: bool,
    pub value: Option<String>,
    pub stdout: String,
    pub stderr: String,
    pub error: Option<String>,
    pub duration_ms: u128,
    /// Age of the kernel process at reply time — lets the tool output say
    /// "same kernel as before" without the model tracking it.
    pub kernel_uptime_secs: u64,
    pub fresh_kernel: bool,
    /// On a fresh kernel: the variables it restored from the scope's
    /// snapshot, so the model knows state carried over instead of assuming
    /// an empty namespace. Always empty on a reused kernel.
    pub restored_vars: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct KernelFrame {
    id: u64,
    ok: bool,
    value: Option<String>,
    error: Option<String>,
    #[serde(default)]
    stdout: String,
    #[serde(default)]
    stderr: String,
    /// Present only on the first frame a kernel emits: the names its boot
    /// restored from the scope's snapshot.
    #[serde(default)]
    restored: Option<Vec<String>>,
}

/// A mid-cell request from kernel code (the `zo` object) to the host. Shaped
/// so it can never be mistaken for a [`KernelFrame`]: it has no `id`/`ok`
/// pair, so a host without bridge support parses nothing and drops the line
/// — the Python side then raises on the closed/ignored channel instead of
/// hanging forever (its read loop only ends on EOF or a reply).
#[derive(Debug, Deserialize)]
struct BridgeFrame {
    zo_bridge: u64,
    op: String,
    #[serde(default)]
    args: serde_json::Value,
}

/// Host-side servicing for kernel `zo.*` calls. The REPL dispatch site
/// supplies one backed by the real tool dispatcher, so a kernel spawn walks
/// the SAME permission check, routing, caps, and manifest bookkeeping as a
/// direct `Agent` tool call — the bridge adds transport, never authority.
pub(crate) trait KernelBridge {
    /// Service one op, returning `{"result": ...}` or `{"error": "..."}` —
    /// the payload handed verbatim to the waiting Python call.
    fn call(&self, op: &str, args: &serde_json::Value) -> serde_json::Value;
}

/// Why one exec failed, typed so the caller can tell "this kernel is dead —
/// respawning is safe" apart from "the USER'S code hung" (a timeout), which
/// must never be silently re-run. String matching here would repeat the
/// overflow-classifier landmine: a message-shape change silently turning a
/// recovery path off.
#[derive(Debug)]
enum KernelError {
    /// The child rejected input or exited: the process is gone, the code
    /// never ran to completion in it, and a fresh kernel may retry safely.
    Dead(String),
    /// The code ran past its deadline. The kernel is discarded, but the same
    /// code must not be auto-retried — it would just hang again.
    Timeout(u128),
}

impl KernelError {
    fn message(&self) -> String {
        match self {
            Self::Dead(detail) => detail.clone(),
            Self::Timeout(ms) => format!("kernel execution exceeded timeout of {ms} ms"),
        }
    }
}

fn registry() -> &'static Mutex<HashMap<String, Arc<Mutex<KernelHandle>>>> {
    static REGISTRY: OnceLock<Mutex<HashMap<String, Arc<Mutex<KernelHandle>>>>> = OnceLock::new();
    REGISTRY.get_or_init(|| Mutex::new(HashMap::new()))
}

/// How long an untouched snapshot file survives before the boot-time sweep
/// removes it. Sessions are mortal; their kernel state should not outlive
/// them by more than a resume-plausible window.
const SNAPSHOT_KEEP: Duration = Duration::from_secs(30 * 24 * 60 * 60);

#[cfg(test)]
thread_local! {
    /// Test seam: snapshots are OFF in tests unless a test opts in with a
    /// directory of its own. The production default would have every kernel
    /// test writing into (and restoring from!) the real config home —
    /// cross-run contamination exactly like the env-inherited todo store.
    static SNAPSHOT_DIR_OVERRIDE: std::cell::RefCell<Option<std::path::PathBuf>> =
        const { std::cell::RefCell::new(None) };
}

#[cfg(test)]
fn set_snapshot_dir_for_this_thread(dir: Option<std::path::PathBuf>) {
    SNAPSHOT_DIR_OVERRIDE.with(|slot| *slot.borrow_mut() = dir);
}

// The Option is the test seam's contract: under cfg(test) this is None until
// a test opts in, so the production arm alone looks unnecessarily wrapped.
#[allow(clippy::unnecessary_wraps)]
fn snapshot_dir() -> Option<std::path::PathBuf> {
    #[cfg(test)]
    {
        SNAPSHOT_DIR_OVERRIDE.with(|slot| slot.borrow().clone())
    }
    #[cfg(not(test))]
    {
        Some(runtime::default_config_home().join("kernel-snapshots"))
    }
}

/// The snapshot file for one scope, with the directory created, locked to
/// the owner, and swept of stale files. Any failure returns `None`: the
/// snapshot is an amenity and must never block an exec.
fn kernel_snapshot_path(scope: &str) -> Option<std::path::PathBuf> {
    let dir = snapshot_dir()?;
    std::fs::create_dir_all(&dir).ok()?;
    let _ = core_types::paths::restrict_permissions_owner_only(&dir);
    prune_stale_snapshots(&dir);
    // Scopes are session ids (uuid-shaped) — sanitize rather than hash so
    // the mapping stays stable across processes and readable on disk.
    let safe: String = scope
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .take(96)
        .collect();
    Some(dir.join(format!("{safe}.pkl")))
}

/// Once per process: drop snapshot files idle past [`SNAPSHOT_KEEP`].
fn prune_stale_snapshots(dir: &std::path::Path) {
    static SWEPT: OnceLock<()> = OnceLock::new();
    SWEPT.get_or_init(|| {
        if let Ok(entries) = std::fs::read_dir(dir) {
            for entry in entries.flatten() {
                let stale = entry
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|modified| modified.elapsed().ok())
                    .is_some_and(|age| age > SNAPSHOT_KEEP);
                if stale {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
    });
}

/// Execute `code` in the persistent kernel for `scope`, spawning one if
/// needed. `reset` discards any existing kernel first. `scope` is the
/// isolation key — callers pass the session id so concurrent sessions (and
/// sub-agents with their own session ids) never share a namespace.
pub(crate) fn execute_persistent_python(
    scope: &str,
    code: &str,
    program: &str,
    cwd: Option<&std::path::Path>,
    timeout: Option<Duration>,
    reset: bool,
    bridge: Option<&dyn KernelBridge>,
) -> Result<KernelExecOutput, ToolError> {
    let timeout = timeout.unwrap_or(DEFAULT_EXEC_TIMEOUT);
    let snapshot = kernel_snapshot_path(scope);
    if reset {
        remove_kernel(scope);
        // Reset means "start clean" — restoring the old namespace from disk
        // right after would betray it, so the snapshot goes with the kernel.
        if let Some(path) = &snapshot {
            let _ = std::fs::remove_file(path);
        }
    }

    // Two attempts, but only for a DEAD kernel: a concurrent call on the same
    // scope may have discarded the kernel between this call taking the Arc and
    // acquiring its mutex (the discarding call holds the mutex first), leaving
    // a killed child whose stdin returns EPIPE. That is transport failure, not
    // a property of the user's code, so one respawn-and-retry heals it. A
    // timeout never retries — the code would just hang again.
    let mut attempted_respawn = false;
    loop {
        let (kernel, fresh_kernel) = {
            let mut map = registry()
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if let Some(existing) = map.get(scope) {
                (Arc::clone(existing), false)
            } else {
                evict_longest_idle_if_full(&mut map);
                let handle = spawn_kernel(program, cwd, snapshot.as_deref())?;
                let handle = Arc::new(Mutex::new(handle));
                map.insert(scope.to_string(), Arc::clone(&handle));
                (handle, true)
            }
        };

        let mut guard = kernel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let started = Instant::now();
        match exec_on_kernel(&mut guard, code, timeout, bridge) {
            Ok(frame) => {
                guard.last_used = Instant::now();
                return Ok(KernelExecOutput {
                    ok: frame.ok,
                    value: frame.value,
                    stdout: cap_stream(frame.stdout),
                    stderr: cap_stream(frame.stderr),
                    error: frame.error,
                    duration_ms: started.elapsed().as_millis(),
                    kernel_uptime_secs: guard.started.elapsed().as_secs(),
                    fresh_kernel,
                    restored_vars: if fresh_kernel {
                        guard.restored_vars.clone()
                    } else {
                        Vec::new()
                    },
                });
            }
            Err(error) => {
                // The kernel is unusable: kill it and unregister so the next
                // call starts clean. Unregister only OUR handle — a concurrent
                // call may already have replaced the map entry with a fresh
                // kernel that must not be torn down.
                let _ = guard.child.kill();
                let _ = guard.child.wait();
                drop(guard);
                remove_kernel_handle(scope, &kernel);

                if matches!(error, KernelError::Dead(_)) && !attempted_respawn {
                    attempted_respawn = true;
                    continue;
                }
                // Truthful loss report: with a snapshot on disk the next
                // kernel restores the picklable variables, and claiming
                // total loss would push the model into needless rebuilds.
                let recovery = match &snapshot {
                    Some(path) if path.exists() => {
                        "the next REPL call starts a fresh kernel and restores \
                         the last snapshot's picklable variables"
                    }
                    _ => {
                        "its variables are lost; the next REPL call starts a \
                         fresh kernel"
                    }
                };
                return Err(ToolError::Execution(format!(
                    "{} — the kernel was discarded; {recovery}",
                    error.message()
                )));
            }
        }
    }
}

/// Drop `scope`'s kernel if present, killing the process.
pub(crate) fn remove_kernel(scope: &str) {
    let handle = {
        let mut map = registry()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.remove(scope)
    };
    if let Some(handle) = handle {
        let mut guard = handle
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _ = guard.child.kill();
        let _ = guard.child.wait();
    }
}

/// Kill and unregister the longest-idle kernel when the registry is at
/// [`MAX_KERNELS`]. Only idle kernels are candidates: a kernel whose mutex is
/// held is mid-exec, and `try_lock` skipping it is what keeps eviction from
/// ever tearing down live work. All-busy at the cap leaves the cap soft — one
/// extra kernel beats blocking a session behind another session's cell.
fn evict_longest_idle_if_full(map: &mut HashMap<String, Arc<Mutex<KernelHandle>>>) {
    if map.len() < MAX_KERNELS {
        return;
    }
    let victim = map
        .iter()
        .filter_map(|(scope, handle)| {
            let guard = handle.try_lock().ok()?;
            Some((scope.clone(), guard.last_used))
        })
        .min_by_key(|(_, last_used)| *last_used)
        .map(|(scope, _)| scope);
    if let Some(scope) = victim {
        if let Some(handle) = map.remove(&scope) {
            // try_lock only: a blocking lock here would hold the REGISTRY
            // mutex while waiting out another call's exec (up to its full
            // timeout), stalling every session's kernel ops. If a racer
            // grabbed the kernel between the probe above and now (it cloned
            // the Arc before we took the map lock), the kernel is merely left
            // unregistered — it idles until zo exits and its stdin EOFs, a
            // bounded leak instead of a global stall.
            if let Ok(mut guard) = handle.try_lock() {
                let _ = guard.child.kill();
                let _ = guard.child.wait();
            }
        }
    }
}

/// Unregister `scope` only if the map still holds exactly `expected` — the
/// compare-and-remove a failing call uses so it never tears down a fresh
/// kernel a concurrent call has already installed in its place.
fn remove_kernel_handle(scope: &str, expected: &Arc<Mutex<KernelHandle>>) {
    let mut map = registry()
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if map
        .get(scope)
        .is_some_and(|current| Arc::ptr_eq(current, expected))
    {
        map.remove(scope);
    }
}

fn spawn_kernel(
    program: &str,
    cwd: Option<&std::path::Path>,
    snapshot: Option<&std::path::Path>,
) -> Result<KernelHandle, ToolError> {
    let mut command = Command::new(program);
    command
        .args(["-u", "-c", PYTHON_BOOTSTRAP])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    // Same scoping rule as the one-shot path: the parent session's todo store
    // is a zo implementation detail and must not leak into children.
    command.env_remove("ZO_TODO_STORE");
    if let Some(snapshot) = snapshot {
        command.env("ZO_KERNEL_SNAPSHOT", snapshot);
    }
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command
        .spawn()
        .map_err(|e| ToolError::Execution(format!("failed to spawn python kernel: {e}")))?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| ToolError::Execution("kernel stdin unavailable".into()))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ToolError::Execution("kernel stdout unavailable".into()))?;

    let (sender, frames) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("zo-repl-kernel-reader".into())
        .spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut line = String::new();
            loop {
                line.clear();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        if sender.send(line.trim_end().to_string()).is_err() {
                            break;
                        }
                    }
                }
            }
        })
        .map_err(|e| ToolError::Execution(format!("failed to start kernel reader: {e}")))?;

    let mut handle = KernelHandle {
        child,
        stdin,
        frames,
        next_request: 0,
        started: Instant::now(),
        last_used: Instant::now(),
        restored_vars: Vec::new(),
    };
    // Readiness probe: a no-op cell proves the interpreter parsed the
    // bootstrap and is serving frames, so a broken install fails here with a
    // clear message instead of as a timeout on the user's first real cell.
    // Its frame also carries the boot-time snapshot-restore report.
    let probe = exec_on_kernel(&mut handle, "None", SPAWN_PROBE_TIMEOUT, None).map_err(|e| {
        let _ = handle.child.kill();
        let _ = handle.child.wait();
        ToolError::Execution(format!(
            "python kernel failed its readiness probe: {}",
            e.message()
        ))
    })?;
    handle.restored_vars = probe.restored.unwrap_or_default();
    Ok(handle)
}

fn exec_on_kernel(
    kernel: &mut KernelHandle,
    code: &str,
    timeout: Duration,
    bridge: Option<&dyn KernelBridge>,
) -> Result<KernelFrame, KernelError> {
    kernel.next_request += 1;
    let id = kernel.next_request;
    let request = json!({ "id": id, "code": code }).to_string();
    kernel
        .stdin
        .write_all(request.as_bytes())
        .and_then(|()| kernel.stdin.write_all(b"\n"))
        .and_then(|()| kernel.stdin.flush())
        .map_err(|e| KernelError::Dead(format!("kernel is not accepting input: {e}")))?;

    let deadline = Instant::now() + timeout;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(KernelError::Timeout(timeout.as_millis()));
        }
        match kernel.frames.recv_timeout(remaining) {
            Ok(line) => {
                // Frames from a request that already timed out are drained and
                // dropped; only the current id answers this call.
                if let Ok(frame) = serde_json::from_str::<KernelFrame>(&line) {
                    if frame.id == id {
                        return Ok(frame);
                    }
                    continue;
                }
                // A mid-cell `zo.*` call. Serviced inline on this thread —
                // the cell is blocked on the reply anyway — and the time it
                // takes stays inside the cell's own deadline, exactly like
                // any other work the cell chooses to do. `zo.spawn` detaches
                // at spawn time, so servicing is bounded, not agent-length.
                if let Ok(frame) = serde_json::from_str::<BridgeFrame>(&line) {
                    let payload = bridge.map_or_else(
                        || json!({"error": "the zo bridge is not available in this REPL context"}),
                        |bridge| bridge.call(&frame.op, &frame.args),
                    );
                    let reply =
                        json!({"zo_bridge": frame.zo_bridge, "payload": payload}).to_string();
                    kernel
                        .stdin
                        .write_all(reply.as_bytes())
                        .and_then(|()| kernel.stdin.write_all(b"\n"))
                        .and_then(|()| kernel.stdin.flush())
                        .map_err(|e| {
                            KernelError::Dead(format!(
                                "kernel stopped accepting bridge replies: {e}"
                            ))
                        })?;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                return Err(KernelError::Timeout(timeout.as_millis()));
            }
            Err(RecvTimeoutError::Disconnected) => {
                let status = kernel
                    .child
                    .try_wait()
                    .ok()
                    .flatten()
                    .map_or_else(|| "unknown".to_string(), |s| s.to_string());
                return Err(KernelError::Dead(format!(
                    "kernel process exited mid-call (status: {status})"
                )));
            }
        }
    }
}

fn cap_stream(mut text: String) -> String {
    if text.len() > MAX_STREAM_BYTES {
        text.truncate(floor_char_boundary(&text, MAX_STREAM_BYTES));
        text.push_str("\n…(output truncated)");
    }
    text
}

/// Largest byte index `<= at` that lands on a char boundary. (Stable-Rust
/// stand-in for `str::floor_char_boundary`.)
fn floor_char_boundary(text: &str, at: usize) -> usize {
    let mut index = at.min(text.len());
    while index > 0 && !text.is_char_boundary(index) {
        index -= 1;
    }
    index
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tests need a real interpreter; every dev/CI box this repo targets has
    /// one. A hard panic (not a silent skip) keeps the suite honest — a
    /// missing python would otherwise turn the whole module into a 0-fire
    /// feature with green tests.
    fn python() -> &'static str {
        super::super::bash_tools::detect_first_command(&["python3", "python"])
            .expect("python3 is required to test the persistent kernel")
    }

    fn scope(name: &str) -> String {
        format!("test-{name}-{}", std::process::id())
    }

    /// Serialize every kernel-spawning test in this module. The registry and
    /// its [`MAX_KERNELS`] LRU cap are process-global, so enough parallel
    /// tests (each with its own scope) evict each other's idle kernels
    /// between two calls of the SAME test — persistence assertions then fail
    /// with `fresh_kernel: true` for a reason that has nothing to do with the
    /// code under test.
    fn kernel_test_lock() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Opt this thread into snapshots under a fresh temp dir; hand back the
    /// dir for cleanup. Snapshots stay OFF for every test that doesn't call
    /// this — the production default writing into the real config home would
    /// contaminate runs through restored state.
    fn snapshot_sandbox(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("zo-kernel-snap-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        set_snapshot_dir_for_this_thread(Some(dir.clone()));
        dir
    }

    fn teardown_snapshot_sandbox(dir: &std::path::Path) {
        set_snapshot_dir_for_this_thread(None);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn snapshot_restores_variables_after_kernel_death() {
        let _serial = kernel_test_lock();
        let dir = snapshot_sandbox("restore");
        let scope = scope("snap-restore");
        execute_persistent_python(&scope, "x = 41", python(), None, None, false, None)
            .expect("seed");
        // os._exit skips the save for THIS cell, so the surviving snapshot is
        // the one from `x = 41` — exactly the durability contract.
        let _ = execute_persistent_python(
            &scope,
            "import os\nos._exit(0)",
            python(),
            None,
            None,
            false,
            None,
        );
        let revived = execute_persistent_python(&scope, "x + 1", python(), None, None, false, None)
            .expect("fresh kernel after death");
        assert!(revived.fresh_kernel);
        assert_eq!(revived.value.as_deref(), Some("42"), "x must survive the dead kernel");
        assert_eq!(revived.restored_vars, vec!["x".to_string()]);
        remove_kernel(&scope);
        teardown_snapshot_sandbox(&dir);
    }

    #[test]
    fn reset_discards_the_snapshot_with_the_kernel() {
        let _serial = kernel_test_lock();
        let dir = snapshot_sandbox("reset");
        let scope = scope("snap-reset");
        execute_persistent_python(&scope, "x = 1", python(), None, None, false, None)
            .expect("seed");
        let cleared =
            execute_persistent_python(&scope, "'x' in dir()", python(), None, None, true, None)
                .expect("reset exec");
        assert!(cleared.fresh_kernel);
        assert_eq!(cleared.value.as_deref(), Some("False"), "reset must not restore");
        assert!(cleared.restored_vars.is_empty());
        remove_kernel(&scope);
        teardown_snapshot_sandbox(&dir);
    }

    #[test]
    fn unpicklable_values_are_skipped_without_losing_the_rest() {
        let _serial = kernel_test_lock();
        let dir = snapshot_sandbox("unpicklable");
        let scope = scope("snap-partial");
        execute_persistent_python(
            &scope,
            "f = open('/dev/null')\ny = 7",
            python(),
            None,
            None,
            false,
            None,
        )
        .expect("seed");
        let _ = execute_persistent_python(
            &scope,
            "import os\nos._exit(0)",
            python(),
            None,
            None,
            false,
            None,
        );
        let revived = execute_persistent_python(
            &scope,
            "('f' in dir(), 'y' in dir(), y)",
            python(),
            None,
            None,
            false,
            None,
        )
        .expect("fresh kernel");
        assert_eq!(
            revived.value.as_deref(),
            Some("(False, True, 7)"),
            "the open file handle is gone, the data survives"
        );
        assert_eq!(revived.restored_vars, vec!["y".to_string()]);
        remove_kernel(&scope);
        teardown_snapshot_sandbox(&dir);
    }

    #[test]
    fn state_persists_across_calls() {
        let _serial = kernel_test_lock();
        let scope = scope("persist");
        let a = execute_persistent_python(&scope, "x = 41", python(), None, None, false, None)
            .expect("first exec");
        assert!(a.ok && a.fresh_kernel);
        let b = execute_persistent_python(&scope, "x + 1", python(), None, None, false, None)
            .expect("second exec");
        assert!(b.ok && !b.fresh_kernel);
        assert_eq!(b.value.as_deref(), Some("42"));
        remove_kernel(&scope);
    }

    #[test]
    fn exception_reports_but_kernel_survives() {
        let _serial = kernel_test_lock();
        let scope = scope("survive");
        execute_persistent_python(&scope, "y = 7", python(), None, None, false, None).expect("seed");
        let boom = execute_persistent_python(&scope, "1/0", python(), None, None, false, None)
            .expect("exception is a result, not a transport error");
        assert!(!boom.ok);
        assert!(
            boom.error.as_deref().unwrap_or("").contains("ZeroDivisionError"),
            "traceback expected: {:?}",
            boom.error
        );
        let after = execute_persistent_python(&scope, "y", python(), None, None, false, None)
            .expect("kernel survives the exception");
        assert_eq!(after.value.as_deref(), Some("7"));
        remove_kernel(&scope);
    }

    #[test]
    fn reset_discards_the_namespace() {
        let _serial = kernel_test_lock();
        let scope = scope("reset");
        execute_persistent_python(&scope, "z = 1", python(), None, None, false, None).expect("seed");
        let fresh = execute_persistent_python(&scope, "'z' in dir()", python(), None, None, true, None)
            .expect("reset exec");
        assert!(fresh.fresh_kernel);
        assert_eq!(fresh.value.as_deref(), Some("False"));
        remove_kernel(&scope);
    }

    #[test]
    fn timeout_discards_kernel_and_next_call_starts_fresh() {
        let _serial = kernel_test_lock();
        let scope = scope("timeout");
        execute_persistent_python(&scope, "w = 5", python(), None, None, false, None).expect("seed");
        let hung = execute_persistent_python(
            &scope,
            "import time\ntime.sleep(60)",
            python(),
            None,
            Some(Duration::from_millis(300)),
            false,
            None,
        );
        let message = hung.expect_err("must time out").to_string();
        assert!(message.contains("timeout"), "{message}");
        assert!(message.contains("variables are lost"), "{message}");
        let fresh = execute_persistent_python(&scope, "'w' in dir()", python(), None, None, false, None)
            .expect("fresh kernel after timeout");
        assert!(fresh.fresh_kernel);
        assert_eq!(fresh.value.as_deref(), Some("False"));
        remove_kernel(&scope);
    }

    #[test]
    fn kernel_death_mid_call_errors_once_then_next_call_is_fresh() {
        let _serial = kernel_test_lock();
        let scope = scope("death");
        execute_persistent_python(&scope, "v = 9", python(), None, None, false, None).expect("seed");
        // `os._exit` kills the kernel before any frame is written. The dead
        // kernel triggers the single respawn-retry, which re-runs the same
        // cell and dies again — so the call errors instead of looping.
        let died = execute_persistent_python(
            &scope,
            "import os\nos._exit(0)",
            python(),
            None,
            None,
            false,
            None,
        );
        let message = died.expect_err("kernel death must surface").to_string();
        assert!(message.contains("exited mid-call"), "{message}");
        assert!(message.contains("variables are lost"), "{message}");

        let fresh = execute_persistent_python(&scope, "'v' in dir()", python(), None, None, false, None)
            .expect("fresh kernel after death");
        assert!(fresh.fresh_kernel);
        assert_eq!(fresh.value.as_deref(), Some("False"));
        remove_kernel(&scope);
    }

    #[test]
    fn scopes_are_isolated() {
        let _serial = kernel_test_lock();
        let left = scope("iso-left");
        let right = scope("iso-right");
        execute_persistent_python(&left, "secret = 'left'", python(), None, None, false, None)
            .expect("left seed");
        let other = execute_persistent_python(&right, "'secret' in dir()", python(), None, None, false, None)
            .expect("right probe");
        assert_eq!(other.value.as_deref(), Some("False"));
        remove_kernel(&left);
        remove_kernel(&right);
    }

    /// Hermetic eviction test on a LOCAL map: filling the global registry to
    /// its cap from a test would evict other parallel tests' kernels and make
    /// the suite flaky.
    #[test]
    fn eviction_kills_the_longest_idle_kernel_and_skips_busy_ones() {
        let _serial = kernel_test_lock();
        let mut map: HashMap<String, Arc<Mutex<KernelHandle>>> = HashMap::new();
        for i in 0..MAX_KERNELS {
            let mut handle = spawn_kernel(python(), None, None).expect("spawn");
            // Deterministic idleness ranking without sleeping: older index =
            // longer idle.
            handle.last_used = Instant::now()
                .checked_sub(Duration::from_secs((MAX_KERNELS - i) as u64))
                .expect("process uptime exceeds a few seconds");
            map.insert(format!("k{i}"), Arc::new(Mutex::new(handle)));
        }

        evict_longest_idle_if_full(&mut map);
        assert_eq!(map.len(), MAX_KERNELS - 1);
        assert!(!map.contains_key("k0"), "longest-idle kernel must be evicted");

        // A busy kernel (mutex held) is never the victim even when longest idle.
        let busy = Arc::clone(map.get("k1").expect("k1 present"));
        let busy_guard = busy.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        // Refill to the cap so eviction triggers again.
        let mut extra = spawn_kernel(python(), None, None).expect("spawn extra");
        extra.last_used = Instant::now();
        map.insert("fresh".into(), Arc::new(Mutex::new(extra)));
        evict_longest_idle_if_full(&mut map);
        assert!(
            map.contains_key("k1"),
            "a mid-exec kernel must be skipped by eviction"
        );
        drop(busy_guard);

        for (_, handle) in map.drain() {
            if let Ok(mut guard) = handle.try_lock() {
                let _ = guard.child.kill();
                let _ = guard.child.wait();
            }
        }
    }

    #[test]
    fn stdout_is_captured_not_interleaved() {
        let _serial = kernel_test_lock();
        let scope = scope("stdout");
        let out = execute_persistent_python(
            &scope,
            "print('hello'); print('world'); 3 * 3",
            python(),
            None,
            None,
            false,
            None,
        )
        .expect("exec");
        assert_eq!(out.stdout, "hello\nworld\n");
        assert_eq!(out.value.as_deref(), Some("9"));
        remove_kernel(&scope);
    }

    #[test]
    fn statement_only_cells_return_no_value() {
        let _serial = kernel_test_lock();
        let scope = scope("stmt");
        let out = execute_persistent_python(&scope, "for _ in range(3): pass", python(), None, None, false, None)
            .expect("exec");
        assert!(out.ok);
        assert_eq!(out.value, None);
        remove_kernel(&scope);
    }

    struct MockBridge {
        calls: Mutex<Vec<(String, serde_json::Value)>>,
    }

    impl KernelBridge for MockBridge {
        fn call(&self, op: &str, args: &serde_json::Value) -> serde_json::Value {
            self.calls
                .lock()
                .expect("mock bridge lock")
                .push((op.to_string(), args.clone()));
            match op {
                "spawn" => json!({"result": {"agentId": "agent-mock-1", "status": "running"}}),
                "result" => json!({"result": {
                    "agentId": args.get("agent_id").cloned().unwrap_or_default(),
                    "status": "completed",
                    "report": "all done",
                }}),
                other => json!({"error": format!("unknown op {other}")}),
            }
        }
    }

    #[test]
    fn zo_bridge_round_trips_spawn_and_result_mid_cell() {
        let _serial = kernel_test_lock();
        let scope = scope("bridge");
        let bridge = MockBridge {
            calls: Mutex::new(Vec::new()),
        };
        let spawned = execute_persistent_python(
            &scope,
            "h = zo.spawn('scan the repository for TODO markers')\nh['agentId']",
            python(),
            None,
            None,
            false,
            Some(&bridge),
        )
        .expect("spawn cell");
        assert!(spawned.ok, "{spawned:?}");
        assert_eq!(spawned.value.as_deref(), Some("'agent-mock-1'"));

        // The handle survives in the namespace; a LATER cell collects the
        // report — the exact two-call shape the tool description teaches.
        let collected = execute_persistent_python(
            &scope,
            "zo.result(h['agentId'])['report']",
            python(),
            None,
            None,
            false,
            Some(&bridge),
        )
        .expect("result cell");
        assert!(collected.ok, "{collected:?}");
        assert_eq!(collected.value.as_deref(), Some("'all done'"));

        let calls = bridge.calls.lock().expect("mock bridge lock");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0, "spawn");
        assert_eq!(
            calls[0].1.get("prompt").and_then(serde_json::Value::as_str),
            Some("scan the repository for TODO markers")
        );
        assert!(
            calls[0]
                .1
                .get("description")
                .and_then(serde_json::Value::as_str)
                .is_some_and(|description| !description.is_empty()),
            "python derives a description when none is given"
        );
        assert_eq!(calls[1].0, "result");
        assert_eq!(
            calls[1].1.get("agent_id").and_then(serde_json::Value::as_str),
            Some("agent-mock-1")
        );
        remove_kernel(&scope);
    }

    #[test]
    fn zo_bridge_error_payload_raises_in_cell_and_kernel_survives() {
        let _serial = kernel_test_lock();
        let scope = scope("bridge-error");
        // No bridge installed: the host replies with an error payload, the
        // Python call raises, and the KERNEL (with its variables) stays alive.
        execute_persistent_python(&scope, "kept = 11", python(), None, None, false, None)
            .expect("seed");
        let denied =
            execute_persistent_python(&scope, "zo.spawn('x')", python(), None, None, false, None)
                .expect("cell runs; the raise is a result, not a transport failure");
        assert!(!denied.ok);
        assert!(
            denied
                .error
                .as_deref()
                .is_some_and(|error| error.contains("not available")),
            "{denied:?}"
        );
        let after = execute_persistent_python(&scope, "kept", python(), None, None, false, None)
            .expect("kernel survives the raised bridge error");
        assert!(after.ok);
        assert_eq!(after.value.as_deref(), Some("11"));
        assert!(!after.fresh_kernel, "the same kernel answered");
        remove_kernel(&scope);
    }
}
