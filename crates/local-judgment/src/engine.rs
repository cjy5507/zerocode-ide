use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStderr, ChildStdin, ChildStdout, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, sync_channel};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use judgment_provider::{Protocol, Provider, ProviderKind};
use serde::Serialize;
use serde_json::{Value, json};

use crate::catalog::{EngineKind, Model, RuntimeProfile};
use crate::storage::child;
use crate::{Error, Result};

pub const MAX_FRAME: usize = 2 * 1024 * 1024;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const START_TIMEOUT: Duration = Duration::from_secs(120);

/// The exit code `bridge.py`'s memory guard ends the runtime with when its
/// resident memory passes the profile's limit.
const MEMORY_LIMIT_EXIT: i32 = 75;

/// How long a runtime whose pipe failed may take to end by itself before the
/// server stops it, so its record says how it ended rather than that it was
/// stopped while ending.
const EXIT_GRACE: Duration = Duration::from_millis(250);

/// How much of its runtime's stderr a server keeps until the runtime is first
/// asked anything: the last lines, each cut short at a word, never more bytes
/// in all.
const STDERR_LINES: usize = 20;
const STDERR_LINE_BYTES: usize = 512;
const STDERR_BYTES: usize = 4 * 1024;

/// How long the record of an ended runtime waits for the last of its stderr.
const STDERR_SETTLE: Duration = Duration::from_millis(500);

/// How often a server looks at its runtime, so its end is told even while
/// another process keeps the runtime's output open.
const WATCH: Duration = Duration::from_millis(200);

/// How many whole seconds a client waits before it asks an ended server whose
/// restart is planned or loading again.
pub(crate) type RetryAfter = Arc<dyn Fn() -> u64 + Send + Sync>;

/// How the owned server admits requests to its runtime: how long one
/// exchange may run, how long a request may wait for its turn, and how many
/// may wait at once.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    pub request: Duration,
    pub queue_wait: Duration,
    pub waiting: usize,
}

impl Limits {
    /// A waiting request never waits longer than one exchange may run, and
    /// the bodies held while waiting never pass the bytes one provider
    /// response may carry.
    pub(crate) const DEFAULT: Self = Self {
        request: REQUEST_TIMEOUT,
        queue_wait: REQUEST_TIMEOUT,
        waiting: judgment_provider::MAX_RESPONSE_BYTES / MAX_FRAME,
    };
}

pub struct Engine {
    child: Mutex<Child>,
    io: Mutex<Pipe>,
    exited: AtomicBool,
    told: AtomicBool,
    on_exit: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
    stopped: Mutex<Option<Stop>>,
    stderr: Arc<Tail>,
}

/// Why this side ended a runtime that was still running.
#[derive(Clone)]
enum Stop {
    /// A request ran past its deadline; the task its body named, if any.
    Deadline(Option<String>),
    /// The pipe to the runtime failed, or the runtime broke its frames.
    Failed,
    /// Its owner retired it: the person's stop, a model change, app cleanup.
    Owner,
}

/// How a runtime ended: the stop this side made or the runtime's own end,
/// the code or signal it ended with, the task a deadline stop's request
/// named, and the last of its stderr, bounded and scrubbed, when it was never
/// asked anything. Of a runtime that was asked, only how many stderr lines it
/// wrote from then on is kept.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Ended {
    pub cause: &'static str,
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub task: Option<String>,
    pub stderr: Vec<String>,
    pub stderr_withheld: u32,
}

/// The last lines a runtime wrote to stderr, read as it writes them so a
/// full pipe never holds it up. Once the runtime is asked anything its stderr
/// may repeat what a person asked, which mask cannot tell from other text, so
/// from then on no line is kept, whenever it drains: lines are only counted.
#[derive(Default)]
struct Tail {
    held: Mutex<Held>,
    closed: Condvar,
}

#[derive(Default)]
struct Held {
    lines: VecDeque<String>,
    bytes: usize,
    closed: bool,
    /// How many lines, blank ones aside, were left out since the runtime was
    /// first asked, once it has been; never reset for its process.
    withheld: Option<u32>,
}

struct Pipe {
    input: ChildStdin,
    output: Receiver<Result<Vec<u8>>>,
}

pub struct Running {
    pub id: String,
    pub model_id: String,
    pub provider: Provider,
    pub engine: Arc<Engine>,
    task: Option<tokio::task::JoinHandle<()>>,
    reservation: Option<std::net::TcpListener>,
    admission: Arc<Admission>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Engine {
    pub fn spawn(
        runtime: &Path,
        profile: &RuntimeProfile,
        model: Option<(&Path, &Model)>,
    ) -> Result<Arc<Self>> {
        if let Some((_, model)) = model {
            crate::adapter::local_profile(model, Some(profile), true)?;
        } else if profile.engine == EngineKind::SystemOneServer {
            return Err(Error::EngineUnsupported);
        }
        for artifact in &profile.runtime_files {
            let path = child(
                runtime,
                &format!("{}/{}", profile.site_packages, artifact.path),
            )?;
            crate::storage::verify(&path, artifact, &AtomicBool::new(false))?;
        }
        let executable = child(runtime, &profile.executable)?;
        let cache = child(runtime, "cache")?;
        crate::storage::private_dir(&cache)?;
        let mut command = Command::new(executable);
        command.args(["-I", "-u", "-c", include_str!("bridge.py")]);
        command.env_clear();
        for name in ["SYSTEMROOT", "WINDIR", "SystemDrive"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command
            .env("HOME", &cache)
            .env("USERPROFILE", &cache)
            .env("TMPDIR", &cache)
            .env("TMP", &cache)
            .env("TEMP", &cache)
            .env("HF_HOME", &cache)
            .env("HF_HUB_OFFLINE", "1")
            .env("TRANSFORMERS_OFFLINE", "1")
            .env("HF_HUB_DISABLE_TELEMETRY", "1")
            .env("TOKENIZERS_PARALLELISM", "false")
            .env("OMP_NUM_THREADS", "2")
            .env("MKL_NUM_THREADS", "2")
            .env("OPENBLAS_NUM_THREADS", "2")
            .env("VECLIB_MAXIMUM_THREADS", "2")
            .env("NUMEXPR_NUM_THREADS", "2")
            .env("DO_NOT_TRACK", "1")
            .env("D3_FAST", "0")
            .env("D3_FUSED", "0")
            .current_dir(runtime)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        let process = command.spawn().map_err(|_| Error::RuntimeFailed)?;
        let engine = Self::from_child(process)?;
        let packages: serde_json::Map<String, Value> = profile
            .wheels
            .iter()
            .filter_map(|wheel| Some((wheel.package.clone()?, json!(wheel.version.as_ref()?))))
            .collect();
        let mut initial = json!({"engine":profile.engine,"mode":"verify","packages":packages});
        initial["rssLimitBytes"] = json!(profile.max_rss_bytes);
        if profile.engine == EngineKind::TransformersSystemOne {
            let template = profile
                .runtime_files
                .iter()
                .find(|artifact| artifact.path == "native-system-one/chat_template.jinja")
                .ok_or(Error::Manifest)?;
            let path = child(
                runtime,
                &format!("{}/{}", profile.site_packages, template.path),
            )?;
            let bytes = crate::storage::bounded_read(&path, 64 * 1024)?;
            initial["chatTemplate"] = json!(String::from_utf8(bytes).map_err(|_| Error::Manifest)?);
        }
        if let Some((directory, model)) = model {
            initial["mode"] = json!("model");
            initial["modelDirectory"] = json!(directory);
            initial["modelId"] = json!(model.id);
            initial["modelFiles"] = json!(model.files);
        }
        let bytes = serde_json::to_vec(&initial).map_err(|_| Error::RuntimeFailed)?;
        engine.write(&bytes)?;
        Ok(engine)
    }

    fn from_child(mut process: Child) -> Result<Arc<Self>> {
        let Some(input) = process.stdin.take() else {
            let _ = process.kill();
            let _ = process.wait();
            return Err(Error::RuntimeFailed);
        };
        let Some(output) = process.stdout.take() else {
            let _ = process.kill();
            let _ = process.wait();
            return Err(Error::RuntimeFailed);
        };
        let errors = process.stderr.take();
        let (sender, receiver) = sync_channel(2);
        let engine = Arc::new(Self {
            child: Mutex::new(process),
            io: Mutex::new(Pipe {
                input,
                output: receiver,
            }),
            exited: AtomicBool::new(false),
            told: AtomicBool::new(false),
            on_exit: Mutex::new(None),
            stopped: Mutex::new(None),
            stderr: Arc::new(Tail::default()),
        });
        match errors {
            Some(errors) => {
                let tail = engine.stderr.clone();
                std::thread::Builder::new()
                    .name("local-judgment-stderr".to_owned())
                    .spawn(move || drain(errors, &tail))
                    .map_err(|_| Error::RuntimeFailed)?;
            }
            None => engine.stderr.close(),
        }
        let weak = Arc::downgrade(&engine);
        std::thread::Builder::new()
            .name("local-judgment-output".to_owned())
            .spawn(move || {
                let mut output = BufReader::new(output);
                loop {
                    let frame = read(&mut output);
                    let failed = frame.is_err();
                    if sender.try_send(frame).is_err() || failed {
                        break;
                    }
                }
                if let Some(engine) = weak.upgrade() {
                    let began = Instant::now();
                    while engine.alive() && began.elapsed() < EXIT_GRACE {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    engine.stop_for(Stop::Failed);
                    engine.tell_exit();
                }
            })
            .map_err(|_| Error::RuntimeFailed)?;
        let weak = Arc::downgrade(&engine);
        std::thread::Builder::new()
            .name("local-judgment-watch".to_owned())
            .spawn(move || {
                loop {
                    std::thread::sleep(WATCH);
                    let Some(engine) = weak.upgrade() else {
                        break;
                    };
                    if !engine.alive() {
                        engine.tell_exit();
                        break;
                    }
                }
            })
            .map_err(|_| Error::RuntimeFailed)?;
        Ok(engine)
    }

    /// Tells the owner once that the runtime has ended, whichever of the
    /// output's end and the watch sees it first.
    fn tell_exit(&self) {
        self.exited.store(true, Ordering::Release);
        if self.told.swap(true, Ordering::AcqRel) {
            return;
        }
        let notify = lock(&self.on_exit).clone();
        if let Some(notify) = notify {
            notify();
        }
    }

    fn on_exit(&self, notify: Arc<dyn Fn() + Send + Sync>) {
        *lock(&self.on_exit) = Some(notify.clone());
        if self.exited.load(Ordering::Acquire) {
            notify();
        }
    }

    fn write(&self, bytes: &[u8]) -> Result<()> {
        let mut io = lock(&self.io);
        io.input
            .write_all(bytes)
            .and_then(|()| io.input.write_all(b"\n"))
            .and_then(|()| io.input.flush())
            .map_err(|_| Error::RuntimeFailed)
    }

    pub fn ready(&self) -> Result<()> {
        let io = lock(&self.io);
        let bytes = io.output.recv().map_err(|_| Error::RuntimeFailed)??;
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| Error::RuntimeFailed)?;
        if value.get("ready").and_then(Value::as_bool) != Some(true) {
            return Err(Error::RuntimeFailed);
        }
        Ok(())
    }

    pub(crate) fn exchange(&self, bytes: &[u8]) -> Result<Vec<u8>> {
        if bytes.len() >= MAX_FRAME {
            return Err(Error::TooLarge);
        }
        self.stderr.withhold();
        let mut io = lock(&self.io);
        io.input
            .write_all(bytes)
            .and_then(|()| io.input.write_all(b"\n"))
            .and_then(|()| io.input.flush())
            .map_err(|_| Error::RuntimeFailed)?;
        io.output.recv().map_err(|_| Error::RuntimeFailed)?
    }

    pub fn alive(&self) -> bool {
        lock(&self.child)
            .try_wait()
            .is_ok_and(|status| status.is_none())
    }

    pub fn stop(&self) {
        self.stop_for(Stop::Owner);
    }

    /// Ends the runtime if it still runs, and keeps why: the first stop made
    /// while it ran is the one its record names.
    fn stop_for(&self, stop: Stop) {
        let mut child = lock(&self.child);
        if child.try_wait().is_ok_and(|status| status.is_none()) {
            lock(&self.stopped).get_or_insert(stop);
            let _ = child.kill();
        }
        let _ = child.wait();
    }

    /// How the runtime ended, once it has. A stop this side made names the
    /// end only when the runtime ended by this side's kill; any other end is
    /// the runtime's own.
    pub(crate) fn ended(&self) -> Option<Ended> {
        let status = lock(&self.child).try_wait().ok().flatten()?;
        let (code, signal) = exit_status(status);
        let stop = lock(&self.stopped)
            .clone()
            .filter(|_| killed_here(code, signal));
        let (cause, task) = match stop {
            Some(Stop::Deadline(task)) => (Error::Timeout, task),
            Some(Stop::Failed) => (Error::RuntimeFailed, None),
            Some(Stop::Owner) => (Error::ServerStopped, None),
            None if code == Some(MEMORY_LIMIT_EXIT) => (Error::MemoryLimit, None),
            None => (Error::RuntimeExited, None),
        };
        let (stderr, stderr_withheld) = self.stderr.settled();
        Some(Ended {
            cause: cause.token(),
            code,
            signal,
            task,
            stderr,
            stderr_withheld,
        })
    }

    #[cfg(all(test, unix))]
    pub(crate) fn from_command(command: &mut Command) -> Arc<Self> {
        // std on macOS marks a new pipe close-on-exec only after making it, so a
        // fixture spawned beside another could keep that one's pipe open.
        static SPAWNING: Mutex<()> = Mutex::new(());
        let child = {
            let _turn = lock(&SPAWNING);
            command
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::piped())
                .spawn()
                .unwrap()
        };
        Self::from_child(child).unwrap()
    }
}

/// The code or the signal a process ended with.
fn exit_status(status: ExitStatus) -> (Option<i32>, Option<i32>) {
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        (status.code(), status.signal())
    }
    #[cfg(not(unix))]
    {
        (status.code(), None)
    }
}

/// Whether an end is the one [`Child::kill`] makes: `SIGKILL` on Unix, and
/// on Windows the exit code it terminates the process with.
#[cfg(unix)]
fn killed_here(_code: Option<i32>, signal: Option<i32>) -> bool {
    signal == Some(libc::SIGKILL)
}

#[cfg(not(unix))]
fn killed_here(code: Option<i32>, _signal: Option<i32>) -> bool {
    code == Some(1)
}

impl Tail {
    /// Keeps one line, cut at its last whole word when it ran past the
    /// length a line may keep, so no value survives in part; the oldest
    /// lines give way past the bounds. Once the tail withholds, a line is
    /// only counted, and only when it would have been kept, judged before
    /// anything is copied.
    fn push(&self, line: &[u8], long: bool) {
        let mut held = lock(&self.held);
        if let Some(withheld) = &mut held.withheld {
            // Blank as judged below: a cut line ends in an ellipsis and a
            // byte that is not UTF-8 reads as U+FFFD, so neither is blank.
            if long
                || line
                    .utf8_chunks()
                    .any(|chunk| !chunk.invalid().is_empty() || !chunk.valid().trim().is_empty())
            {
                *withheld = withheld.saturating_add(1);
            }
            return;
        }
        let mut line = String::from_utf8_lossy(line).into_owned();
        if long {
            let cut = line
                .char_indices()
                .rfind(|&(at, ch)| ch.is_whitespace() && at + '…'.len_utf8() <= STDERR_LINE_BYTES)
                .map_or(0, |(at, _)| at);
            line.truncate(cut);
            line.push('…');
        }
        if line.trim().is_empty() {
            return;
        }
        held.bytes += line.len();
        held.lines.push_back(line);
        while held.lines.len() > STDERR_LINES || held.bytes > STDERR_BYTES {
            let Some(oldest) = held.lines.pop_front() else {
                break;
            };
            held.bytes -= oldest.len();
        }
    }

    fn close(&self) {
        lock(&self.held).closed = true;
        self.closed.notify_all();
    }

    /// Stops keeping lines for good and lets go of those held, before the
    /// runtime is first asked anything, as no request can be told apart from
    /// a person's. A line the runtime began earlier but ends later is judged
    /// as it ends, so it is withheld too.
    fn withhold(&self) {
        let mut held = lock(&self.held);
        if held.withheld.is_none() {
            held.withheld = Some(0);
            held.lines = VecDeque::new();
            held.bytes = 0;
        }
    }

    /// The kept lines and how many were withheld, once stderr has closed or
    /// [`STDERR_SETTLE`] has passed: the lines with their private values
    /// masked by the table the release gate reads, and still within the
    /// bounds after masking.
    fn settled(&self) -> (Vec<String>, u32) {
        let held = self
            .closed
            .wait_timeout_while(lock(&self.held), STDERR_SETTLE, |held| !held.closed)
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .0;
        let withheld = held.withheld.unwrap_or(0);
        let mut lines: Vec<String> = held
            .lines
            .iter()
            .map(|line| {
                let mut masked = zerocode_core::private_data::mask(line).text;
                let mut end = masked.len().min(STDERR_LINE_BYTES);
                while !masked.is_char_boundary(end) {
                    end -= 1;
                }
                masked.truncate(end);
                masked
            })
            .collect();
        drop(held);
        let mut bytes = 0;
        let kept = lines
            .iter()
            .rev()
            .take_while(|line| {
                bytes += line.len();
                bytes <= STDERR_BYTES
            })
            .count();
        lines.drain(..lines.len() - kept);
        (lines, withheld)
    }
}

/// Reads a runtime's stderr until it closes, keeping only its tail.
fn drain(mut errors: ChildStderr, tail: &Tail) {
    use std::io::Read;
    let mut buffer = [0; 8 * 1024];
    let mut line = Vec::with_capacity(STDERR_LINE_BYTES);
    let mut long = false;
    loop {
        let read = match errors.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        for &byte in &buffer[..read] {
            if byte == b'\n' || byte == b'\r' {
                tail.push(&line, long);
                line.clear();
                long = false;
            } else if line.len() < STDERR_LINE_BYTES {
                line.push(byte);
            } else {
                long = true;
            }
        }
    }
    tail.push(&line, long);
    tail.close();
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

fn read(output: &mut BufReader<ChildStdout>) -> Result<Vec<u8>> {
    use std::io::Read;
    let mut bytes = Vec::new();
    output
        .take(MAX_FRAME as u64 + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|_| Error::RuntimeFailed)?;
    if bytes.len() > MAX_FRAME {
        return Err(Error::TooLarge);
    }
    if bytes.last() != Some(&b'\n') {
        return Err(Error::RuntimeFailed);
    }
    bytes.pop();
    Ok(bytes)
}

/// The requests one owned server holds. The runtime reads one frame and
/// answers it before it reads the next (`bridge.py`'s single loop, and
/// [`Engine::exchange`] holding the pipe across both), so one request runs
/// at a time and at most `limits.waiting` wait for their turn behind it. A
/// request past that is refused at once; a closed admission refuses every
/// request still waiting.
struct Admission {
    turn: Arc<tokio::sync::Semaphore>,
    admitted: AtomicUsize,
    limits: Limits,
    restarting: Mutex<Option<RetryAfter>>,
}

/// One admitted request's place, given back when it is dropped: when the
/// request is answered or refused, or its caller leaves while it waits.
struct Ticket(Arc<Admission>);

impl Admission {
    /// How many requests the runtime answers at once.
    const RUNNING: usize = 1;

    fn new(limits: Limits) -> Arc<Self> {
        Arc::new(Self {
            turn: Arc::new(tokio::sync::Semaphore::new(Self::RUNNING)),
            admitted: AtomicUsize::new(0),
            limits,
            restarting: Mutex::new(None),
        })
    }

    /// The Retry-After an ended server answers with while its restart is
    /// planned or loading.
    fn retry_after(&self) -> Option<u64> {
        let after = lock(&self.restarting).clone();
        after.map(|after| after())
    }

    fn enter(self: &Arc<Self>) -> Option<Ticket> {
        self.admitted
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |held| {
                (held < Self::RUNNING + self.limits.waiting).then_some(held + 1)
            })
            .ok()
            .map(|_| Ticket(self.clone()))
    }

    fn close(&self) {
        self.turn.close();
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        self.0.admitted.fetch_sub(1, Ordering::AcqRel);
    }
}

#[derive(Clone)]
struct HttpState {
    engine: Arc<Engine>,
    provider: Provider,
    admission: Arc<Admission>,
    failed: Arc<dyn Fn() + Send + Sync>,
}

fn browser_request(headers: &HeaderMap, provider: &Provider) -> bool {
    let host = provider.base_url.trim_start_matches("http://");
    headers.contains_key("origin")
        || headers.contains_key("sec-fetch-site")
        || headers.get("host").and_then(|value| value.to_str().ok()) != Some(host)
}

fn failure(status: StatusCode, error: Error) -> Response {
    (status, axum::Json(json!({"error":error.token()}))).into_response()
}

fn retry_later(seconds: u64) -> Response {
    // A kept-alive connection would stay on the ended server after its
    // restart; closing it sends the client's next request to the port anew.
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [
            (axum::http::header::RETRY_AFTER, seconds.to_string()),
            (axum::http::header::CONNECTION, "close".to_owned()),
        ],
        axum::Json(json!({"error":Error::Restarting.token()})),
    )
        .into_response()
}

async fn models(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    if browser_request(&headers, &state.provider) {
        return failure(StatusCode::FORBIDDEN, Error::Protocol);
    }
    if let Some(seconds) = state.admission.retry_after() {
        return retry_later(seconds);
    }
    if !state.engine.alive() {
        return failure(StatusCode::SERVICE_UNAVAILABLE, Error::ServerStopped);
    }
    axum::Json(json!({"data":[{"id":state.provider.model}]})).into_response()
}

async fn predict(State(state): State<HttpState>, headers: HeaderMap, body: Bytes) -> Response {
    if browser_request(&headers, &state.provider) {
        return failure(StatusCode::FORBIDDEN, Error::Protocol);
    }
    if let Some(seconds) = state.admission.retry_after() {
        return retry_later(seconds);
    }
    let Some(ticket) = state.admission.enter() else {
        return failure(StatusCode::SERVICE_UNAVAILABLE, Error::Busy);
    };
    let mut request: Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return failure(StatusCode::BAD_REQUEST, Error::Protocol),
    };
    if request.get("model").and_then(Value::as_str) != Some(&state.provider.model) {
        return failure(StatusCode::BAD_REQUEST, Error::UnknownModel);
    }
    if request.get("images").is_some() || request.get("videos").is_some() {
        return failure(StatusCode::UNPROCESSABLE_ENTITY, Error::InputUnsupported);
    }
    if request
        .get("questions")
        .and_then(Value::as_object)
        .is_some_and(|questions| questions.len() > 32)
    {
        return failure(StatusCode::PAYLOAD_TOO_LARGE, Error::TooLarge);
    }
    request["model"] = json!(state.provider.identity());
    let mut prepared = match serde_json::to_vec(&request)
        .ok()
        .and_then(|bytes| state.provider.prepare(bytes).ok())
    {
        Some(prepared) => prepared,
        None => return failure(StatusCode::BAD_REQUEST, Error::Protocol),
    };
    if prepared.body.len() >= MAX_FRAME {
        return failure(StatusCode::PAYLOAD_TOO_LARGE, Error::TooLarge);
    }
    let named = task_name(&request);
    drop(request);
    drop(body);
    let turn = match tokio::time::timeout(
        state.admission.limits.queue_wait,
        state.admission.turn.clone().acquire_owned(),
    )
    .await
    {
        Ok(Ok(turn)) => turn,
        Ok(Err(_)) => return failure(StatusCode::SERVICE_UNAVAILABLE, Error::ServerStopped),
        Err(_) => return failure(StatusCode::SERVICE_UNAVAILABLE, Error::Busy),
    };
    let bytes = std::mem::take(&mut prepared.body);
    match tokio::spawn(exchange_in_turn(state.clone(), turn, ticket, bytes, named)).await {
        Ok(Ok(bytes)) => match state.provider.decode(&prepared, bytes.clone()) {
            Ok(_) => (
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                bytes,
            )
                .into_response(),
            Err(refusal) => (
                StatusCode::BAD_GATEWAY,
                axum::Json(json!({"error":Error::Protocol.token(),"refusal":refusal.token()})),
            )
                .into_response(),
        },
        Ok(Err(error)) => failure(StatusCode::SERVICE_UNAVAILABLE, error),
        Err(_) => failure(StatusCode::SERVICE_UNAVAILABLE, Error::RuntimeFailed),
    }
}

/// The task a request names in its body, kept with a deadline stop so the
/// next stop says which seat sent it: a short plain name, never free text,
/// and never a name the private-value table would mask, as a key's shape
/// fits a name's characters.
fn task_name(request: &Value) -> Option<String> {
    request
        .get("task")
        .and_then(Value::as_str)
        .filter(|task| {
            (1..=64).contains(&task.len())
                && task
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || b"._:-".contains(&byte))
                && zerocode_core::private_data::mask(task).text == *task
        })
        .map(str::to_owned)
}

/// One request's exchange with the runtime, holding its turn, bounded by the
/// request deadline. A runtime that misses the deadline or fails is stopped,
/// and the requests waiting behind it are refused rather than handed to it.
async fn exchange_in_turn(
    state: HttpState,
    turn: tokio::sync::OwnedSemaphorePermit,
    ticket: Ticket,
    bytes: Vec<u8>,
    named: Option<String>,
) -> Result<Vec<u8>> {
    let _ticket = ticket;
    let engine = state.engine.clone();
    let admission = state.admission.clone();
    let task = tokio::task::spawn_blocking(move || {
        let _turn = turn;
        let result = engine.exchange(&bytes);
        if result.is_err() {
            admission.close();
        }
        result
    });
    let (error, stop) = match tokio::time::timeout(state.admission.limits.request, task).await {
        Ok(Ok(Ok(bytes))) => return Ok(bytes),
        Err(_) => (Error::Timeout, Stop::Deadline(named)),
        Ok(_) => {
            let began = Instant::now();
            while state.engine.alive() && began.elapsed() < EXIT_GRACE {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            (Error::RuntimeFailed, Stop::Failed)
        }
    };
    state.admission.close();
    state.engine.stop_for(stop);
    (state.failed)();
    Err(error)
}

#[cfg(all(test, unix))]
pub async fn serve(
    model: &Model,
    engine: Arc<Engine>,
    failed: Arc<dyn Fn() + Send + Sync>,
) -> Result<Running> {
    serve_on(model, engine, failed, None, Limits::DEFAULT).await
}

#[cfg(all(test, unix))]
pub(crate) async fn serve_limited(
    model: &Model,
    engine: Arc<Engine>,
    failed: Arc<dyn Fn() + Send + Sync>,
    limits: Limits,
) -> Result<Running> {
    serve_with(model, engine, failed, None, limits).await
}

pub(crate) async fn serve_on(
    model: &Model,
    engine: Arc<Engine>,
    failed: Arc<dyn Fn() + Send + Sync>,
    listener: Option<std::net::TcpListener>,
    limits: Limits,
) -> Result<Running> {
    serve_with(model, engine, failed, listener, limits).await
}

async fn serve_with(
    model: &Model,
    engine: Arc<Engine>,
    failed: Arc<dyn Fn() + Send + Sync>,
    listener: Option<std::net::TcpListener>,
    limits: Limits,
) -> Result<Running> {
    let admission = Admission::new(limits);
    let closing = admission.clone();
    let exited = failed.clone();
    engine.on_exit(Arc::new(move || {
        closing.close();
        exited();
    }));
    match model.engine {
        EngineKind::Laya | EngineKind::TransformersSystemOne => {}
        EngineKind::SystemOneServer => return Err(Error::EngineUnsupported),
    }
    let listener = match listener {
        Some(listener) => listener,
        None => tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(|_| Error::RuntimeFailed)?
            .into_std()
            .map_err(|_| Error::RuntimeFailed)?,
    };
    let address = listener.local_addr().map_err(|_| Error::RuntimeFailed)?;
    if address.ip() != std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST) || address.port() == 0 {
        return Err(Error::Protocol);
    }
    listener
        .set_nonblocking(true)
        .map_err(|_| Error::RuntimeFailed)?;
    for _ in 0..32 {
        match listener.accept() {
            Ok((stream, _)) => drop(stream),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(_) => return Err(Error::RuntimeFailed),
        }
    }
    let reservation = listener.try_clone().map_err(|_| Error::RuntimeFailed)?;
    let listener = tokio::net::TcpListener::from_std(listener).map_err(|_| Error::RuntimeFailed)?;
    let port = listener
        .local_addr()
        .map_err(|_| Error::RuntimeFailed)?
        .port();
    let provider = Provider {
        kind: ProviderKind::Local,
        protocol: Protocol::SystemOne,
        base_url: format!("http://127.0.0.1:{port}"),
        model: model.id.clone(),
        requires_auth: false,
    };
    let state = HttpState {
        engine: engine.clone(),
        provider: provider.clone(),
        admission: admission.clone(),
        failed,
    };
    let router = axum::Router::new()
        .route("/v1/models", get(models))
        .route("/v1/systemone", post(predict))
        .layer(DefaultBodyLimit::max(MAX_FRAME))
        .with_state(state);
    let task = tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });
    Ok(Running {
        id: uuid::Uuid::new_v4().to_string(),
        model_id: model.id.clone(),
        provider,
        engine,
        task: Some(task),
        reservation: Some(reservation),
        admission,
    })
}

impl Running {
    /// Requests the server holds right now: the one its runtime is answering
    /// and the ones waiting for their turn.
    #[cfg(all(test, unix))]
    pub(crate) fn admitted(&self) -> usize {
        self.admission.admitted.load(Ordering::Acquire)
    }

    pub(crate) fn take_listener(&mut self) -> Result<std::net::TcpListener> {
        self.reservation.take().ok_or(Error::RuntimeFailed)
    }

    /// Answers every request from here on with `server_restarting` and the
    /// Retry-After `after` gives: this server's runtime ended, and its restart
    /// is planned or loading.
    pub(crate) fn restarting(&self, after: RetryAfter) {
        *lock(&self.admission.restarting) = Some(after);
    }

    pub(crate) async fn retire(mut self) {
        self.admission.close();
        self.engine.stop();
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
        self.admission.close();
        self.engine.stop();
    }
}

#[cfg(all(test, unix))]
#[path = "engine_boundary_tests.rs"]
mod boundary_tests;
