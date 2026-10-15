use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, sync_channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use judgment_provider::{Protocol, Provider, ProviderKind};
use serde_json::{Value, json};

use crate::catalog::{EngineKind, Model, RuntimeProfile};
use crate::storage::child;
use crate::{Error, Result};

pub const MAX_FRAME: usize = 2 * 1024 * 1024;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub const START_TIMEOUT: Duration = Duration::from_secs(120);

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
    on_exit: Mutex<Option<Arc<dyn Fn() + Send + Sync>>>,
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
            .stderr(Stdio::null());
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
        let (sender, receiver) = sync_channel(2);
        let engine = Arc::new(Self {
            child: Mutex::new(process),
            io: Mutex::new(Pipe {
                input,
                output: receiver,
            }),
            exited: AtomicBool::new(false),
            on_exit: Mutex::new(None),
        });
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
                    engine.stop();
                    engine.exited.store(true, Ordering::Release);
                    let notify = lock(&engine.on_exit).clone();
                    if let Some(notify) = notify {
                        notify();
                    }
                }
            })
            .map_err(|_| Error::RuntimeFailed)?;
        Ok(engine)
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
        let mut child = lock(&self.child);
        if child.try_wait().is_ok_and(|status| status.is_none()) {
            let _ = child.kill();
        }
        let _ = child.wait();
    }

    #[cfg(all(test, unix))]
    pub(crate) fn from_command(command: &mut Command) -> Arc<Self> {
        let child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        Self::from_child(child).unwrap()
    }
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
        })
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

async fn models(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    if browser_request(&headers, &state.provider) {
        return failure(StatusCode::FORBIDDEN, Error::Protocol);
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
    match tokio::spawn(exchange_in_turn(state.clone(), turn, ticket, bytes)).await {
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

/// One request's exchange with the runtime, holding its turn, bounded by the
/// request deadline. A runtime that misses the deadline or fails is stopped,
/// and the requests waiting behind it are refused rather than handed to it.
async fn exchange_in_turn(
    state: HttpState,
    turn: tokio::sync::OwnedSemaphorePermit,
    ticket: Ticket,
    bytes: Vec<u8>,
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
    let error = match tokio::time::timeout(state.admission.limits.request, task).await {
        Ok(Ok(Ok(bytes))) => return Ok(bytes),
        Err(_) => Error::Timeout,
        Ok(_) => Error::RuntimeFailed,
    };
    state.admission.close();
    state.engine.stop();
    (state.failed)();
    Err(error)
}

#[cfg(all(test, unix))]
pub async fn serve(
    model: &Model,
    engine: Arc<Engine>,
    failed: Arc<dyn Fn() + Send + Sync>,
) -> Result<Running> {
    serve_on(model, engine, failed, None).await
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

pub async fn serve_on(
    model: &Model,
    engine: Arc<Engine>,
    failed: Arc<dyn Fn() + Send + Sync>,
    listener: Option<std::net::TcpListener>,
) -> Result<Running> {
    serve_with(model, engine, failed, listener, Limits::DEFAULT).await
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
