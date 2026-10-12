use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
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
    task: tokio::task::JoinHandle<()>,
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
        if let Some((directory, model)) = model {
            initial["mode"] = json!("model");
            initial["modelDirectory"] = json!(directory);
            initial["modelId"] = json!(model.id);
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

    fn exchange(&self, bytes: &[u8]) -> Result<Vec<u8>> {
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

#[derive(Clone)]
struct HttpState {
    engine: Arc<Engine>,
    provider: Provider,
    permit: Arc<tokio::sync::Semaphore>,
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
    let Ok(permit) = state.permit.clone().try_acquire_owned() else {
        return failure(StatusCode::SERVICE_UNAVAILABLE, Error::Busy);
    };
    let mut request: Value = match serde_json::from_slice(&body) {
        Ok(request) => request,
        Err(_) => return failure(StatusCode::BAD_REQUEST, Error::Protocol),
    };
    if request.get("model").and_then(Value::as_str) != Some(&state.provider.model) {
        return failure(StatusCode::BAD_REQUEST, Error::UnknownModel);
    }
    if request
        .get("questions")
        .and_then(Value::as_object)
        .is_some_and(|questions| questions.len() > 32)
    {
        return failure(StatusCode::PAYLOAD_TOO_LARGE, Error::TooLarge);
    }
    request["model"] = json!(state.provider.identity());
    let prepared = match serde_json::to_vec(&request)
        .ok()
        .and_then(|bytes| state.provider.prepare(bytes).ok())
    {
        Some(prepared) => prepared,
        None => return failure(StatusCode::BAD_REQUEST, Error::Protocol),
    };
    let engine = state.engine.clone();
    let bytes = prepared.body.clone();
    let mut task = tokio::task::spawn_blocking(move || {
        let _permit = permit;
        engine.exchange(&bytes)
    });
    let result = tokio::time::timeout(REQUEST_TIMEOUT, &mut task).await;
    match result {
        Ok(Ok(Ok(bytes))) => {
            if state.provider.decode(&prepared, bytes.clone()).is_err() {
                return failure(StatusCode::BAD_GATEWAY, Error::Protocol);
            }
            (
                [(axum::http::header::CONTENT_TYPE, "application/json")],
                bytes,
            )
                .into_response()
        }
        other => {
            state.engine.stop();
            (state.failed)();
            let error = if other.is_err() {
                Error::Timeout
            } else {
                Error::RuntimeFailed
            };
            failure(StatusCode::SERVICE_UNAVAILABLE, error)
        }
    }
}

pub async fn serve(
    model: &Model,
    engine: Arc<Engine>,
    failed: Arc<dyn Fn() + Send + Sync>,
) -> Result<Running> {
    engine.on_exit(failed.clone());
    match model.engine {
        EngineKind::Laya => {}
    }
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
        .await
        .map_err(|_| Error::RuntimeFailed)?;
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
        permit: Arc::new(tokio::sync::Semaphore::new(1)),
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
        task,
    })
}

impl Drop for Running {
    fn drop(&mut self) {
        self.task.abort();
        self.engine.stop();
    }
}
