//! `zo serve` — a persistent local session server.
//!
//! ## Why
//!
//! A plain `zo` REPL is a single process: when the terminal closes, the SSH
//! pipe drops, or the laptop sleeps, the in-memory conversation dies with it.
//! `zo serve` lifts the session pool out of the foreground process. The
//! server owns a `HashMap<session_id, LiveCli>` behind locks and survives client
//! comings-and-goings; `zo attach <id>` (see [`crate::attach`]) connects over
//! a TCP socket, replays history, and streams turns — detach and reattach
//! without losing state.
//!
//! ## Reuse, not rewrite
//!
//! The server is a thin transport shell around the *existing* headless core:
//!
//! - Each session is a [`LiveCli`] built exactly as `zo -p` builds one, so
//!   every subsystem (plugins, MCP, LSP, permissions, persistence) comes along
//!   for free.
//! - A turn runs through
//!   [`LiveCli::run_turn_streaming_to_channel`](crate::session::LiveCli), which
//!   reuses the same `run_turn_streaming_with_images` path the interactive TUI
//!   drives — only the [`RenderBlock`] sink differs (a socket instead of the
//!   terminal).
//! - The wire format is line-delimited JSON-RPC (see [`crate::serve_protocol`]);
//!   the JSON-RPC envelopes mirror the `mcp_stdio` shapes already in the tree,
//!   so no new protocol crate is pulled in.
//!
//! ## Concurrency model
//!
//! A **multi-thread** Tokio runtime drives an accept loop; each connection is a
//! spawned task. Session state is shared as
//! `Arc<Mutex<HashMap<id, Arc<tokio::sync::Mutex<LiveCli>>>>>`:
//!
//! - the outer `std::sync::Mutex` guards the map and is held only for the
//!   microsecond of a lookup/insert (never across `.await`);
//! - the inner `tokio::sync::Mutex` serializes turns **per session** — two
//!   clients poking the same session can't interleave a turn, but distinct
//!   sessions run fully concurrently.
//!
//! Multi-thread is required, not incidental: `build_runtime` (session
//! construction) trips a nested-runtime panic if called on an async worker, so
//! sessions are built on a `spawn_blocking` thread — which also forces the
//! `LiveCli: Send` bound asserted below.
//!
//! ## Permission forwarding (F2)
//!
//! Turn permission prompts are **forwarded to the attached client**: a
//! [`SocketPermissionPrompter`](crate::session::socket_permission) streams each
//! gate as a `permission_prompt` render frame and parks a responder keyed by a
//! server-global `prompt_id` (in the shared [`SocketPrompterConfig`]). The
//! client shows a modal and answers with a `permission.respond` RPC on a second
//! connection; [`dispatch_permission_respond`] resolves the parked responder,
//! unblocking the turn. An unanswered prompt hard-denies after a timeout, so a
//! vanished client never wedges a turn. Sessions persist to the project's
//! `.zo/sessions/` and are **rehydrated into the pool on restart** (see
//! [`rehydrate_persisted_sessions`]), so ids created before a bounce keep
//! resolving for `session.load`/`run_turn`.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::tcp::OwnedWriteHalf;
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot};

use runtime::message_stream::RenderBlock;
use runtime::PermissionMode;

use crate::cli_args::AllowedToolSet;
use crate::serve::pair::{PairHub, SteerAuth, SubscribeError};
use crate::serve_auth::{AuthOutcome, ServeAuthPolicy, ServeCapability};
use crate::serve_protocol::{
    CancelTurnParams, HistoryEntry, JobIdParams, PermissionRespondParams, RpcRequest, RpcResponse,
    RunTurnDetachedParams, RunTurnParams, SessionIdParams, SessionInfo, SessionSummary, SteerParams,
    SubscribeParams,
    CODE_CANCELLED, CODE_HELM_HELD, CODE_INTERNAL, CODE_INVALID_PARAMS, CODE_INVALID_REQUEST,
    CODE_METHOD_NOT_FOUND, CODE_NO_SUCH_SESSION, CODE_STEER_DENIED, CODE_UNAUTHORIZED,
};
use crate::session::socket_permission::SocketPrompterConfig;
use crate::session::LiveCli;
use crate::session_registry::SessionScope;
use runtime::message_stream::BlockIdGen;
use zo_cli::sinks::permission_decision_from_tag;

mod pair;

/// Compile-time guard: a session must be `Send` so it can be built on a
/// `spawn_blocking` worker and shared across the multi-thread runtime's tasks.
/// If a future change drops a non-`Send` field into the runtime tower, this
/// fails to compile with a precise pointer at the offending type rather than a
/// confusing `tokio::spawn` error deep in this module.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<LiveCli>();
};

/// Shared, mutable pool of live sessions keyed by session id.
type SessionMap = Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<LiveCli>>>>>;

/// Shared registry of in-flight cancellable turns (F4), keyed by
/// `(session_id, turn_id)`. `session.run_turn` registers a oneshot sender while
/// a turn runs; a `session.cancel_turn` (on a second connection) fires it, which
/// raises the runtime's explicit abort signal. Scoping the key by session id keeps
/// share a `turn_id` from cancelling each other; a legacy `cancel_turn` that
/// omits the session id is honoured only when the `turn_id` is unambiguous
/// (see [`dispatch_cancel_turn`]).
type CancelMap = Arc<Mutex<HashMap<(String, u64), tokio::sync::oneshot::Sender<()>>>>;

/// Background detached turns keyed by server-assigned job id.
type JobMap = Arc<Mutex<HashMap<u64, JobHandle>>>;

const JOB_TTL: Duration = Duration::from_secs(60 * 60);
const MAX_JOBS: usize = 128;
const MAX_JOB_FRAMES: usize = 2048;

static NEXT_JOB_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum JobStatus {
    Running,
    Done,
    Error,
    Cancelled,
}

impl JobStatus {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Done => "done",
            Self::Error => "error",
            Self::Cancelled => "cancelled",
        }
    }

    const fn is_terminal(self) -> bool {
        !matches!(self, Self::Running)
    }
}

#[derive(Debug)]
struct JobHandle {
    session_id: String,
    started_at: Instant,
    completed_at: Option<Instant>,
    status: JobStatus,
    frames: VecDeque<serde_json::Value>,
    result: Option<serde_json::Value>,
    error: Option<String>,
}

impl JobHandle {
    fn running(session_id: String, now: Instant) -> Self {
        Self {
            session_id,
            started_at: now,
            completed_at: None,
            status: JobStatus::Running,
            frames: VecDeque::new(),
            result: None,
            error: None,
        }
    }

    fn push_frame(&mut self, frame: serde_json::Value) {
        if self.frames.len() == MAX_JOB_FRAMES {
            self.frames.pop_front();
        }
        self.frames.push_back(frame);
    }

    fn finish_success(&mut self, result: serde_json::Value, now: Instant) {
        self.status = JobStatus::Done;
        self.completed_at = Some(now);
        self.result = Some(result);
        self.error = None;
    }

    fn finish_error(&mut self, status: JobStatus, error: String, now: Instant) {
        debug_assert!(status.is_terminal());
        self.status = status;
        self.completed_at = Some(now);
        self.result = None;
        self.error = Some(error);
    }

    fn is_expired(&self, now: Instant) -> bool {
        self.completed_at
            .is_some_and(|completed| now.duration_since(completed) >= JOB_TTL)
    }

    fn status_json(&self, job_id: u64) -> serde_json::Value {
        let age_ms = u64::try_from(self.started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
        serde_json::json!({
            "job_id": job_id,
            "session_id": self.session_id,
            "status": self.status.as_str(),
            "frame_count": self.frames.len(),
            "done": self.status.is_terminal(),
            "age_ms": age_ms,
        })
    }

    fn result_json(&self, job_id: u64) -> serde_json::Value {
        let mut value = self.status_json(job_id);
        if let Some(map) = value.as_object_mut() {
            map.insert(
                "frames".to_string(),
                serde_json::Value::Array(self.frames.iter().cloned().collect()),
            );
            if let Some(result) = &self.result {
                map.insert("result".to_string(), result.clone());
            }
            if let Some(error) = &self.error {
                map.insert(
                    "error".to_string(),
                    serde_json::Value::String(error.clone()),
                );
            }
        }
        value
    }
}

/// Immutable per-server configuration applied to every session it creates.
struct ServeConfig {
    model: String,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
    /// Shared-secret policy required on requests, or open for a tokenless
    /// (loopback-only) server. Read once from the serve auth env at startup.
    auth: ServeAuthPolicy,
}

/// Entry point for `zo serve`. Builds a multi-thread Tokio runtime and runs
/// the accept loop until the process is interrupted (Ctrl-C) or `bind` fails.
pub(crate) fn run_serve(
    bind_addr: String,
    model: String,
    allowed_tools: Option<AllowedToolSet>,
    permission_mode: PermissionMode,
) -> Result<(), Box<dyn std::error::Error>> {
    // Auth gate (G21): refuse to expose an unauthenticated server on a
    // network-reachable address. A pure decision, checked before we open any
    // socket so the failure is immediate and the message explains the fix.
    let auth = ServeAuthPolicy::from_env();
    if let Err(refusal) = crate::serve_auth::startup_gate_for_auth(&bind_addr, auth.has_any_token())
    {
        return Err(refusal.into());
    }
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        // Must match the live TUI runtime's generosity (session/mod.rs): every
        // ordinary tool runs through `spawn_blocking`, and a served session's
        // formatter/keychain/tool work all share this pool. At 8 threads a
        // handful of concurrent agents starved the pool and queued every
        // subsequent tool dispatch behind multi-second formatter runs.
        .max_blocking_threads(512)
        .thread_name("zo-serve")
        .enable_all()
        .build()?;
    let config = Arc::new(ServeConfig {
        model,
        allowed_tools,
        permission_mode,
        auth,
    });
    runtime.block_on(serve_loop(bind_addr, config))
}

/// Bind the listener and accept connections forever, racing Ctrl-C for a clean
/// shutdown message. Each accepted socket is handled on its own task.
async fn serve_loop(
    bind_addr: String,
    config: Arc<ServeConfig>,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = TcpListener::bind(&bind_addr)
        .await
        .map_err(|error| format!("zo serve: failed to bind {bind_addr}: {error}"))?;
    let sessions: SessionMap = Arc::new(Mutex::new(HashMap::new()));
    let cancels: CancelMap = Arc::new(Mutex::new(HashMap::new()));
    let jobs: JobMap = Arc::new(Mutex::new(HashMap::new()));
    // F2: one server-wide responder map + prompt-id space shared by every turn's
    // socket prompter and the `permission.respond` handler.
    let permission = SocketPrompterConfig::new();
    // Track 5: one server-wide pair-session hub — frame fan-out, subscriber
    // registry, per-session helm/steering, and the peer roster.
    let hub = PairHub::default();

    // Restore persisted Project sessions so ids created before a restart keep
    // working (`session.load`/`run_turn` resume their transcripts). Building a
    // `LiveCli` runs the runtime tower, which panics on an async worker, so do
    // it on a blocking thread — exactly like `dispatch_create`.
    let rehydrate_config = Arc::clone(&config);
    let restored =
        tokio::task::spawn_blocking(move || rehydrate_persisted_sessions(&rehydrate_config))
            .await
            .unwrap_or_else(|join_error| {
                eprintln!("[serve] rehydration task panicked: {join_error}");
                Vec::new()
            });
    let restored_count = restored.len();
    {
        let mut pool = sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for (id, cli) in restored {
            pool.insert(id, cli);
        }
    }

    println!(
        "zo serve listening on {bind_addr} (model {})",
        config.model
    );
    println!("  attach with:  zo attach <session-id> --bind {bind_addr}");
    println!("  Ctrl-C to stop. Sessions persist to .zo/sessions/.");
    println!("  rehydrated {restored_count} session(s) from .zo/sessions/.");
    if config.auth.has_any_token() {
        println!("  auth: shared-secret token required (clients read ZO_SERVE_TOKEN).");
        if config.auth.has_read_token() {
            println!("  auth: read-only token also configured via ZO_SERVE_READ_TOKEN.");
        }
    }

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                let (stream, peer) = match accepted {
                    Ok(pair) => pair,
                    Err(error) => {
                        eprintln!("[serve] accept error: {error}");
                        continue;
                    }
                };
                let sessions = sessions.clone();
                let cancels = cancels.clone();
                let jobs = jobs.clone();
                let permission = permission.clone();
                let hub = hub.clone();
                let config = config.clone();
                tokio::spawn(async move {
                    if let Err(error) =
                        handle_conn(stream, sessions, cancels, jobs, permission, hub, config).await
                    {
                        eprintln!("[serve] connection {peer} closed: {error}");
                    }
                });
            }
            _ = tokio::signal::ctrl_c() => {
                let count = sessions.lock().unwrap_or_else(std::sync::PoisonError::into_inner).len();
                println!("\nzo serve: shutting down ({count} session(s) persisted).");
                return Ok(());
            }
        }
    }
}

/// Read JSON-RPC request lines off `stream` and dispatch each until EOF.
///
/// All socket writes for this connection funnel through one [`ConnWriter`]
/// drained by a dedicated writer task ([`spawn_conn_writer`]), so a background
/// subscription can push fan-out frames onto the connection while the read loop
/// answers requests — without interleaving bytes. The connection lazily
/// registers an authenticated roster peer and tears its subscriptions down on
/// exit.
async fn handle_conn(
    stream: TcpStream,
    sessions: SessionMap,
    cancels: CancelMap,
    jobs: JobMap,
    permission: SocketPrompterConfig,
    hub: PairHub,
    config: Arc<ServeConfig>,
) -> std::io::Result<()> {
    let (read_half, socket_write) = stream.into_split();
    let (out_tx, out_rx) = mpsc::channel::<Arc<str>>(pair::OUT_CHANNEL_CAP);
    let conn_id = pair::next_conn_id();
    let writer = ConnWriter::new(out_tx);
    let (writer_done_tx, writer_done_rx) = oneshot::channel();
    let write_task = spawn_conn_writer(socket_write, out_rx, writer_done_tx);
    let result = Box::pin(conn_read_loop(
        read_half,
        &sessions,
        &cancels,
        &jobs,
        &permission,
        &hub,
        &config,
        conn_id,
        &writer,
        writer_done_rx,
    ))
    .await;

    // Tear down: drop this connection's subscriptions/peer entry (refreshing the
    // rosters of every session it watched), then close the funnel so the writer
    // task drains and exits.
    hub.remove_peer(conn_id);
    drop(writer);
    // Subscriber-held writer clones must be released by remove_peer/close.
    // Bound shutdown even if an unexpected clone survives.
    let mut write_task = write_task;
    if tokio::time::timeout(std::time::Duration::from_secs(5), &mut write_task)
        .await
        .is_err()
    {
        write_task.abort();
        let _ = write_task.await;
    }
    result
}

/// The request read loop, split out so `handle_conn` can always run the peer
/// teardown regardless of how the loop ended.
#[allow(clippy::too_many_arguments)]
async fn conn_read_loop(
    read_half: tokio::net::tcp::OwnedReadHalf,
    sessions: &SessionMap,
    cancels: &CancelMap,
    jobs: &JobMap,
    permission: &SocketPrompterConfig,
    hub: &PairHub,
    config: &ServeConfig,
    conn_id: u64,
    writer: &ConnWriter,
    mut writer_done: oneshot::Receiver<()>,
) -> std::io::Result<()> {
    let mut lines = BufReader::new(read_half).lines();
    loop {
        let next_line = tokio::select! {
            line = lines.next_line() => line?,
            _ = &mut writer_done => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "connection writer exited",
                ));
            }
        };
        let Some(line) = next_line else {
            return Ok(());
        };
        if line.trim().is_empty() {
            continue;
        }
        let request: RpcRequest = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(error) => {
                // Malformed frame: answer with id 0 so a client still gets a
                // structured error rather than a silent hang.
                write_response(
                    writer,
                    RpcResponse::err(0, CODE_INVALID_PARAMS, format!("invalid request: {error}")),
                )
                .await?;
                continue;
            }
        };
        // Boxed: the dispatch future crossed clippy's 16 KiB stack-size bound
        // when the conversation runtime grew its per-turn contract slots; one
        // heap allocation per RPC is noise next to the socket round-trip.
        Box::pin(dispatch(
            &request, sessions, cancels, jobs, permission, hub, config, conn_id, writer,
            &mut writer_done,
        ))
        .await?;
    }
}

/// Route one request to its handler.
#[allow(clippy::too_many_arguments)]
async fn dispatch(
    request: &RpcRequest,
    sessions: &SessionMap,
    cancels: &CancelMap,
    jobs: &JobMap,
    permission: &SocketPrompterConfig,
    hub: &PairHub,
    config: &ServeConfig,
    conn_id: u64,
    writer: &ConnWriter,
    writer_done: &mut oneshot::Receiver<()>,
) -> std::io::Result<()> {
    if request.jsonrpc != "2.0" {
        return write_response(
            writer,
            RpcResponse::err(
                request.id,
                CODE_INVALID_REQUEST,
                "jsonrpc must be \"2.0\"",
            ),
        )
        .await;
    }

    // Auth gate (G21): when the server runs with a shared secret, every request
    // must present a matching token before it reaches any handler. Tokenless
    // (loopback) servers skip this — `authorize` returns `Allowed` for `None`.
    let required_capability = required_capability(request.method.as_str());
    match config
        .auth
        .authorize(request.token.as_deref(), required_capability)
    {
        AuthOutcome::Allowed => {
            // Roster membership is lazy: unauthenticated connections and the
            // fire-and-forget cancel/permission sidecars never mutate it. The
            // original anon label remains stable after this first insertion.
            if !matches!(request.method.as_str(), "session.cancel_turn" | "permission.respond") {
                hub.ensure_peer(conn_id, pair::next_anon_label(), required_capability);
            }
        }
        AuthOutcome::Rejected => {
            return write_response(
                writer,
                RpcResponse::err(
                    request.id,
                    CODE_UNAUTHORIZED,
                    "unauthorized: missing or invalid token (set ZO_SERVE_TOKEN to match the server)",
                ),
            )
            .await;
        }
        AuthOutcome::InsufficientCapability => {
            return write_response(
                writer,
                RpcResponse::err(
                    request.id,
                    CODE_UNAUTHORIZED,
                    "unauthorized: token lacks capability for this method",
                ),
            )
            .await;
        }
    }
    match request.method.as_str() {
        "session.create" => Box::pin(dispatch_create(request, sessions, config, writer)).await,
        "session.list" => dispatch_list(request, sessions, writer).await,
        "session.load" => dispatch_load(request, sessions, writer).await,
        "session.info" => dispatch_info(request, sessions, writer).await,
        "session.close" => dispatch_close(request, sessions, hub, writer).await,
        "session.run_turn" => {
            dispatch_run_turn(request, sessions, cancels, permission, hub, conn_id, writer).await
        }
        "session.run_turn_detached" => {
            dispatch_run_turn_detached(request, sessions, cancels, jobs, permission, hub, writer)
                .await
        }
        "session.subscribe" => {
            dispatch_subscribe(request, sessions, hub, conn_id, writer, writer_done).await
        },
        "session.unsubscribe" => dispatch_unsubscribe(request, hub, conn_id, writer).await,
        "session.steer" => dispatch_steer(request, hub, writer).await,
        "session.roster" => dispatch_roster(request, hub, writer).await,
        "session.job_status" => dispatch_job_status(request, jobs, writer).await,
        "session.job_result" => dispatch_job_result(request, jobs, writer).await,
        "session.cancel_turn" => dispatch_cancel_turn(request, cancels, writer).await,
        "session.commit_push_pr" => dispatch_commit_push_pr(request, sessions, writer).await,
        "permission.respond" => dispatch_permission_respond(request, permission, writer).await,
        "session.set_model" => dispatch_set_model(request, sessions, writer).await,
        "session.set_permission" => dispatch_set_permission(request, sessions, writer).await,
        "session.connect_api_key" => dispatch_connect_api_key(request, sessions, writer).await,
        "session.connect_custom_provider" => {
            dispatch_connect_custom_provider(request, sessions, writer).await
        }
        "session.select_session" => dispatch_select_session(request, sessions, writer).await,
        "session.rewind_checkpoint" => {
            dispatch_rewind_checkpoint(request, sessions, writer).await
        }
        other => {
            write_response(
                writer,
                RpcResponse::err(
                    request.id,
                    CODE_METHOD_NOT_FOUND,
                    format!("unknown method: {other}"),
                ),
            )
            .await
        }
    }
}

fn required_capability(method: &str) -> ServeCapability {
    match method {
        // Read = spectate: inspect and watch, but never drive. `subscribe`/
        // `unsubscribe`/`roster` join the existing read tier so a read-only
        // token can follow a session live (track 5 §2.7).
        "session.list" | "session.load" | "session.info" | "session.job_status"
        | "session.job_result" | "session.subscribe" | "session.unsubscribe"
        | "session.roster" => ServeCapability::Read,
        // Full = steer: run_turn / steer / cancel / permission.respond / set_*.
        _ => ServeCapability::Full,
    }
}

/// Restore every persisted Project session into live `LiveCli` instances for
/// the pool. **Synchronous and must run inside `spawn_blocking`** — building a
/// `LiveCli` constructs the runtime tower, which panics on an async worker.
///
/// Honest degradation: a single unreadable/corrupt transcript is logged and
/// skipped (the server still boots with the rest); enumeration failure logs and
/// returns empty. Project-scope only, so one-shot ephemeral runs are never
/// revived into the long-lived pool.
fn rehydrate_persisted_sessions(
    config: &ServeConfig,
) -> Vec<(String, Arc<tokio::sync::Mutex<LiveCli>>)> {
    let files = match crate::session_registry::project_session_files() {
        Ok(files) => files,
        Err(error) => {
            eprintln!("[serve] could not enumerate persisted sessions: {error}");
            return Vec::new();
        }
    };
    let mut restored = Vec::new();
    for path in files {
        let session = match runtime::Session::load_from_path(&path) {
            Ok(session) => session,
            Err(error) => {
                eprintln!(
                    "[serve] skipping unreadable session {}: {error}",
                    path.display()
                );
                continue;
            }
        };
        let id = session.session_id.clone();
        match LiveCli::new_scoped_with_session(
            session,
            config.model.clone(),
            true,
            config.allowed_tools.clone(),
            config.permission_mode,
            SessionScope::Project,
        ) {
            Ok(cli) => restored.push((id, Arc::new(tokio::sync::Mutex::new(cli)))),
            Err(error) => eprintln!("[serve] skipping session {id}: {error}"),
        }
    }
    restored
}

/// `session.create` → build a fresh session and return its id.
async fn dispatch_create(
    request: &RpcRequest,
    sessions: &SessionMap,
    config: &ServeConfig,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let model = config.model.clone();
    let allowed_tools = config.allowed_tools.clone();
    let permission_mode = config.permission_mode;
    // `LiveCli::new_scoped` builds the full runtime tower, which panics if run
    // on an async worker (nested runtime). Build it on a blocking thread.
    let built = tokio::task::spawn_blocking(move || {
        LiveCli::new_scoped(
            model,
            true,
            allowed_tools,
            permission_mode,
            SessionScope::Project,
        )
        .map_err(|error| error.to_string())
    })
    .await;

    let cli = match built {
        Ok(Ok(cli)) => cli,
        Ok(Err(error)) => {
            return write_response(
                writer,
                RpcResponse::err(
                    request.id,
                    CODE_INTERNAL,
                    format!("session.create failed: {error}"),
                ),
            )
            .await;
        }
        Err(join_error) => {
            return write_response(
                writer,
                RpcResponse::err(
                    request.id,
                    CODE_INTERNAL,
                    format!("session.create panicked: {join_error}"),
                ),
            )
            .await;
        }
    };

    let id = cli.session.id.clone();
    sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .insert(id.clone(), Arc::new(tokio::sync::Mutex::new(cli)));
    write_response(
        writer,
        RpcResponse::ok(request.id, serde_json::json!({ "id": id })),
    )
    .await
}

/// `session.list` → every live session id and its message count.
async fn dispatch_list(
    request: &RpcRequest,
    sessions: &SessionMap,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    // Snapshot the (id, handle) pairs, dropping the map lock before any `.await`
    // so a long-running turn never blocks the accept path.
    let entries: Vec<(String, Arc<tokio::sync::Mutex<LiveCli>>)> = {
        let map = sessions
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        map.iter()
            .map(|(id, handle)| (id.clone(), handle.clone()))
            .collect()
    };
    let mut summaries = Vec::with_capacity(entries.len());
    for (id, handle) in entries {
        let messages = handle.lock().await.runtime.session().messages.len();
        summaries.push(SessionSummary { id, messages });
    }
    summaries.sort_by(|a, b| a.id.cmp(&b.id));
    write_response(
        writer,
        RpcResponse::ok(request.id, serde_json::json!({ "sessions": summaries })),
    )
    .await
}

/// `session.load` → the session's conversation history for an attaching client
/// to replay.
async fn dispatch_load(
    request: &RpcRequest,
    sessions: &SessionMap,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let params: SessionIdParams = match serde_json::from_value(request.params.clone()) {
        Ok(params) => params,
        Err(error) => {
            return write_response(
                writer,
                RpcResponse::err(
                    request.id,
                    CODE_INVALID_PARAMS,
                    format!("invalid params: {error}"),
                ),
            )
            .await;
        }
    };
    let Some(handle) = lookup(sessions, &params.id) else {
        return write_response(
            writer,
            RpcResponse::err(
                request.id,
                CODE_NO_SUCH_SESSION,
                format!("no such session: {}", params.id),
            ),
        )
        .await;
    };
    let history = match acquire_live_guard(sessions, &params.id, &handle).await {
        Ok(guard) => project_history(guard.runtime.session()),
        Err(()) => return write_no_such_session(request, &params.id, writer).await,
    };
    write_response(
        writer,
        RpcResponse::ok(
            request.id,
            serde_json::json!({ "id": params.id, "history": history }),
        ),
    )
    .await
}

/// `session.close` → persist and remove a live session from the serve pool.
async fn dispatch_close(
    request: &RpcRequest,
    sessions: &SessionMap,
    hub: &PairHub,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let Ok(params) = parse_params::<SessionIdParams>(request, writer).await else {
        return Ok(());
    };
    let Some(handle) = lookup(sessions, &params.id) else {
        return write_response(
            writer,
            RpcResponse::err(request.id, CODE_NO_SUCH_SESSION, format!("no such session: {}", params.id)),
        )
        .await;
    };

    // Keep this exact guard across map removal and fan-out cleanup. A competing
    // subscribe may retain an Arc, but it cannot lock/register it after this
    // point; map identity is checked by subscribe while holding its guard.
    let guard = handle.lock().await;
    if let Err(error) = guard.persist_session().map_err(|error| error.to_string()) {
        return write_response(
            writer,
            RpcResponse::err(
                request.id,
                CODE_INTERNAL,
                format!("session.close failed to persist {}: {error}", params.id),
            ),
        )
        .await;
    }
    let closed = {
        let mut map = sessions.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if session_handle_is_current(&map, &params.id, &handle) {
            map.remove(&params.id);
            true
        } else {
            false
        }
    };
    if closed {
        hub.close_session(&params.id);
    }
    drop(guard);
    write_response(
        writer,
        RpcResponse::ok(request.id, serde_json::json!({ "id": params.id, "closed": closed })),
    )
    .await
}

/// `session.info` → session metadata (model, permission mode, cwd, branch) so
/// an attaching TUI client can hydrate its sidebar without a local runtime.
async fn dispatch_info(
    request: &RpcRequest,
    sessions: &SessionMap,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let params: SessionIdParams = match serde_json::from_value(request.params.clone()) {
        Ok(params) => params,
        Err(error) => {
            return write_response(
                writer,
                RpcResponse::err(
                    request.id,
                    CODE_INVALID_PARAMS,
                    format!("invalid params: {error}"),
                ),
            )
            .await;
        }
    };
    let Some(handle) = lookup(sessions, &params.id) else {
        return write_response(
            writer,
            RpcResponse::err(
                request.id,
                CODE_NO_SUCH_SESSION,
                format!("no such session: {}", params.id),
            ),
        )
        .await;
    };
    let (model, permission_mode, cwd_path) = match acquire_live_guard(sessions, &params.id, &handle).await {
        Ok(guard) => (
            guard.model.clone(),
            guard.permission_mode,
            guard.cwd.clone(),
        ),
        Err(()) => return write_no_such_session(request, &params.id, writer).await,
    };
    // Resolve the session's git branch against its own workspace cwd (P2: was
    // hard-coded `None`). Shells out to `git`, so it runs on a blocking thread to
    // keep the reactor responsive; an unresolved branch degrades to `None`.
    let branch_cwd = cwd_path.clone();
    let git_branch = tokio::task::spawn_blocking(move || {
        crate::git_helpers::resolve_git_branch_for(&branch_cwd)
    })
    .await
    .unwrap_or(None);
    let info = SessionInfo {
        id: params.id,
        model,
        permission_mode: permission_label(permission_mode).to_string(),
        cwd: cwd_path.display().to_string(),
        git_branch,
    };
    let result = serde_json::to_value(&info).unwrap_or(serde_json::Value::Null);
    write_response(writer, RpcResponse::ok(request.id, result)).await
}

// --- F3: AppAction → session-mutating RPCs -----------------------------------
//
// These meta-operations mutate live session state (model/permission/active
// session/rewind) the way the local REPL's pickers do. They follow the
// `dispatch_info` lookup pattern, but with one critical difference: three of the
// four rebuild the runtime tower or shell out to `git`, which **panics on an
// async worker** (nested runtime) or would block the reactor. So they run on a
// blocking thread via `spawn_blocking` + `blocking_lock` — the same constraint
// `dispatch_create` and rehydration honor. `set_model` only swaps the client's
// model field (no rebuild), so it stays on the async lock.
//
// Editor / clipboard / mouse-capture stay **client-local** (the server has no
// terminal or OS clipboard) — see `attach_tui`.

#[derive(serde::Deserialize)]
struct SetModelParams {
    id: String,
    model: String,
}

#[derive(serde::Deserialize)]
struct SetPermissionParams {
    id: String,
    mode: String,
}

#[derive(serde::Deserialize)]
struct ConnectApiKeyParams {
    id: String,
    provider: String,
    api_key: String,
}

#[derive(serde::Deserialize)]
struct ConnectCustomProviderParams {
    id: String,
    name: String,
    base_url: String,
    #[serde(default)]
    auth_env: Option<String>,
    #[serde(default)]
    api_key: Option<String>,
    #[serde(default)]
    models: Vec<String>,
    #[serde(default)]
    context_window: Option<u64>,
    #[serde(default)]
    max_output_tokens: Option<u64>,
    #[serde(default)]
    include_usage: bool,
    /// Whether this endpoint's models take OpenAI's `reasoning_effort`.
    /// Defaults `false` — an older client that omits it keeps the safe wire
    /// shape rather than sending a field the endpoint may reject.
    #[serde(default)]
    supports_reasoning_effort: bool,
    /// `true` when the attached TUI submitted the wizard in edit mode, so the
    /// fields replace the stored entry instead of merging into it. Defaults
    /// `false`, which keeps older clients on the additive path.
    #[serde(default)]
    edit_existing: bool,
}

#[derive(serde::Deserialize)]
struct SelectSessionParams {
    id: String,
    #[serde(default)]
    session_path: Option<String>,
}

/// Parse `params` into `T`, or write the standard invalid-params error.
async fn parse_params<T: serde::de::DeserializeOwned>(
    request: &RpcRequest,
    writer: &ConnWriter,
) -> Result<T, ()> {
    match serde_json::from_value::<T>(request.params.clone()) {
        Ok(value) => Ok(value),
        Err(error) => {
            let _ = write_response(
                writer,
                RpcResponse::err(
                    request.id,
                    CODE_INVALID_PARAMS,
                    format!("invalid params: {error}"),
                ),
            )
            .await;
            Err(())
        }
    }
}

/// Look up a session or write the standard no-such-session error.
async fn require_session(
    request: &RpcRequest,
    sessions: &SessionMap,
    id: &str,
    writer: &ConnWriter,
) -> Result<Arc<tokio::sync::Mutex<LiveCli>>, ()> {
    if let Some(handle) = lookup(sessions, id) {
        return Ok(handle);
    }
    let _ = write_response(
        writer,
        RpcResponse::err(
            request.id,
            CODE_NO_SUCH_SESSION,
            format!("no such session: {id}"),
        ),
    )
    .await;
    Err(())
}

/// Lock `handle` and linearize the operation against `session.close`. A handle
/// cloned before close is not live once its map entry no longer has this Arc.
async fn acquire_live_guard<'a>(
    sessions: &SessionMap,
    id: &str,
    handle: &'a Arc<tokio::sync::Mutex<LiveCli>>,
) -> Result<tokio::sync::MutexGuard<'a, LiveCli>, ()> {
    let guard = handle.lock().await;
    if session_is_current(sessions, id, handle) {
        Ok(guard)
    } else {
        drop(guard);
        Err(())
    }
}

/// Run a synchronous session metadata operation without moving an async mutex
/// guard across the blocking-thread boundary. The identity check happens only
/// after `blocking_lock()` succeeds, so `session.close` cannot remove this Arc
/// between a preliminary check and the side effect.
async fn run_blocking_meta<R, F>(
    sessions: &SessionMap,
    id: &str,
    handle: Arc<tokio::sync::Mutex<LiveCli>>,
    f: F,
) -> Result<R, String>
where
    R: Send + 'static,
    F: FnOnce(&mut LiveCli) -> R + Send + 'static,
{
    let sessions = Arc::clone(sessions);
    let id = id.to_string();
    tokio::task::spawn_blocking(move || {
        let mut guard = handle.blocking_lock();
        if !session_is_current(&sessions, &id, &handle) {
            return Err(format!("no such session: {id}"));
        }
        Ok(f(&mut guard))
    })
    .await
    .map_err(|join_error| format!("meta operation task panicked: {join_error}"))?
}

async fn write_no_such_session(
    request: &RpcRequest,
    id: &str,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    write_response(
        writer,
        RpcResponse::err(request.id, CODE_NO_SUCH_SESSION, format!("no such session: {id}")),
    )
    .await
}

/// `session.set_model` → swap the active model (no runtime rebuild, so the
/// async lock is safe). Returns the same human-readable report the REPL shows.
async fn dispatch_set_model(
    request: &RpcRequest,
    sessions: &SessionMap,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let Ok(params) = parse_params::<SetModelParams>(request, writer).await else {
        return Ok(());
    };
    let Ok(handle) = require_session(request, sessions, &params.id, writer).await else {
        return Ok(());
    };
    let report = match acquire_live_guard(sessions, &params.id, &handle).await {
        Ok(mut guard) => guard.apply_model_change(&params.model),
        Err(()) => return write_no_such_session(request, &params.id, writer).await,
    };
    write_response(
        writer,
        RpcResponse::ok(request.id, serde_json::json!({ "message": report })),
    )
    .await
}

/// `session.commit_push_pr` → run the commit → push → PR flow against the
/// session's own workspace cwd, the same sequence as the local `/commit-push-pr`
/// slash command. Shells out to `git`/`gh`, so it runs on a blocking thread; any
/// failure is folded into the human-readable report string.
async fn dispatch_commit_push_pr(
    request: &RpcRequest,
    sessions: &SessionMap,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let Ok(params) = parse_params::<SessionIdParams>(request, writer).await else {
        return Ok(());
    };
    let Ok(handle) = require_session(request, sessions, &params.id, writer).await else {
        return Ok(());
    };
    let report = match run_blocking_meta(sessions, &params.id, handle, |guard| {
        crate::session::handle_commit_push_pr_at(&guard.cwd)
    })
    .await
    {
        Ok(report) => report,
        Err(reason) => {
            return write_response(
                writer,
                RpcResponse::err(request.id, CODE_NO_SUCH_SESSION, reason),
            )
            .await;
        }
    };
    write_response(
        writer,
        RpcResponse::ok(request.id, serde_json::json!({ "message": report })),
    )
    .await
}

/// `session.set_permission` → switch the permission tier. Rebuilds the runtime,
/// so it runs on a blocking thread; the domain error maps to an RPC error.
async fn dispatch_set_permission(
    request: &RpcRequest,
    sessions: &SessionMap,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let Ok(params) = parse_params::<SetPermissionParams>(request, writer).await else {
        return Ok(());
    };
    let Ok(handle) = require_session(request, sessions, &params.id, writer).await else {
        return Ok(());
    };
    // Stringify the error *inside* the blocking closure: `Box<dyn Error>` is not
    // `Send`, so it cannot cross the `spawn_blocking` boundary.
    let outcome = match run_blocking_meta(sessions, &params.id, handle, move |guard| {
        guard
            .apply_permission_change(&params.mode)
            .map_err(|error| error.to_string())
    })
    .await
    {
        Ok(outcome) => outcome,
        Err(reason) => {
            return write_response(
                writer,
                RpcResponse::err(request.id, CODE_NO_SUCH_SESSION, reason),
            )
            .await;
        }
    };
    write_meta_outcome(request.id, outcome, writer).await
}

/// `session.connect_api_key` → save an OpenAI-compatible cloud adapter API key
/// and register the provider, matching the local TUI `/connect` API-key modal.
async fn dispatch_connect_api_key(
    request: &RpcRequest,
    sessions: &SessionMap,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let Ok(params) = parse_params::<ConnectApiKeyParams>(request, writer).await else {
        return Ok(());
    };
    let Ok(_handle) = require_session(request, sessions, &params.id, writer).await else {
        return Ok(());
    };
    let outcome: Result<String, String> = tokio::task::spawn_blocking(move || {
        use crate::session::slash_dispatch::{ConnectReport, connect_preset_with_api_key};
        match connect_preset_with_api_key(&params.provider, &params.api_key) {
            ConnectReport::Info(message) | ConnectReport::Warn(message) => Ok(message),
            ConnectReport::Error(message) => Err(message),
        }
    })
    .await
    .unwrap_or_else(|join_error| Err(format!("connect API-key task panicked: {join_error}")));
    write_meta_outcome(request.id, outcome, writer).await
}

/// `session.connect_custom_provider` → save a guided custom OpenAI-compatible
/// provider from either local TUI or an attached TUI.
async fn dispatch_connect_custom_provider(
    request: &RpcRequest,
    sessions: &SessionMap,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let Ok(params) = parse_params::<ConnectCustomProviderParams>(request, writer).await else {
        return Ok(());
    };
    let Ok(_handle) = require_session(request, sessions, &params.id, writer).await else {
        return Ok(());
    };
    let outcome: Result<String, String> = tokio::task::spawn_blocking(move || {
        use crate::session::slash_dispatch::{
            ConnectReport, CustomProviderRequest, ProviderTokenLimits, connect_custom_provider,
        };
        match connect_custom_provider(&CustomProviderRequest {
            name: &params.name,
            base_url: &params.base_url,
            auth_env: params.auth_env.as_deref(),
            api_key: params.api_key.as_deref(),
            models: &params.models,
            token_limits: ProviderTokenLimits {
                context_window: params.context_window,
                max_output_tokens: params.max_output_tokens,
            },
            include_usage: params.include_usage,
            supports_reasoning_effort: params.supports_reasoning_effort,
            edit_existing: params.edit_existing,
        }) {
            ConnectReport::Info(message) | ConnectReport::Warn(message) => Ok(message),
            ConnectReport::Error(message) => Err(message),
        }
    })
    .await
    .unwrap_or_else(|join_error| Err(format!("connect custom provider task panicked: {join_error}")));
    write_meta_outcome(request.id, outcome, writer).await
}

/// `session.select_session` → resume a different persisted transcript into this
/// live session. Rebuilds the runtime (blocking thread). The client follows up
/// with `session.load` to reseed its rendered transcript.
async fn dispatch_select_session(
    request: &RpcRequest,
    sessions: &SessionMap,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let Ok(params) = parse_params::<SelectSessionParams>(request, writer).await else {
        return Ok(());
    };
    let Ok(handle) = require_session(request, sessions, &params.id, writer).await else {
        return Ok(());
    };
    let outcome = match run_blocking_meta(sessions, &params.id, handle, move |guard| {
        guard
            .resume_session_fast(params.session_path.as_deref())
            .map_err(|error| error.to_string())
    })
    .await
    {
        Ok(outcome) => outcome,
        Err(reason) => {
            return write_response(
                writer,
                RpcResponse::err(request.id, CODE_NO_SUCH_SESSION, reason),
            )
            .await;
        }
    };
    write_meta_outcome(request.id, outcome, writer).await
}

/// `session.rewind_checkpoint` → undo the previous turn's conversation + code
/// together. Shells out to `git` for the snapshot undo, so it runs on a blocking
/// thread to keep the reactor responsive.
async fn dispatch_rewind_checkpoint(
    request: &RpcRequest,
    sessions: &SessionMap,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let Ok(params) = parse_params::<SessionIdParams>(request, writer).await else {
        return Ok(());
    };
    let Ok(handle) = require_session(request, sessions, &params.id, writer).await else {
        return Ok(());
    };
    let report = match run_blocking_meta(sessions, &params.id, handle, |guard| {
        let report = guard.rewind_last_checkpoint();
        // Format inside the closure into a plain, `Send` shape.
        (report.is_noop(), report.messages_removed)
    })
    .await
    {
        Ok(report) => report,
        Err(reason) => {
            return write_response(
                writer,
                RpcResponse::err(request.id, CODE_NO_SUCH_SESSION, reason),
            )
            .await;
        }
    };
    let (noop, removed) = report;
    let message = if noop {
        "nothing to rewind".to_string()
    } else {
        format!("rewound {removed} message(s) and the code snapshot")
    };
    write_response(
        writer,
        RpcResponse::ok(
            request.id,
            serde_json::json!({
                "message": message,
                "messages_removed": removed,
                "noop": noop,
            }),
        ),
    )
    .await
}

/// Write a `Result<String, String>` meta-operation outcome as an RPC
/// success (`{ "message": … }`) or an internal error.
async fn write_meta_outcome(
    id: u64,
    outcome: Result<String, String>,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    match outcome {
        Ok(message) => {
            write_response(
                writer,
                RpcResponse::ok(id, serde_json::json!({ "message": message })),
            )
            .await
        }
        Err(reason) => {
            write_response(writer, RpcResponse::err(id, CODE_INTERNAL, reason)).await
        }
    }
}

/// Canonical CLI label for a permission mode, round-tripped by the client
/// through `permission_mode_from_label`. `Prompt`/`Allow` fold onto their
/// nearest labelled tier (the three the client parses).
fn permission_label(mode: PermissionMode) -> &'static str {
    match mode {
        PermissionMode::ReadOnly => "read-only",
        PermissionMode::WorkspaceWrite | PermissionMode::Prompt => "workspace-write",
        PermissionMode::DangerFullAccess | PermissionMode::Allow => "danger-full-access",
    }
}

/// One winning branch per `dispatch_run_turn` select loop, returned so the
/// borrow of `block_rx` inside `recv()` is dropped *before* the match body
/// closes the receiver (avoids a select-arm double borrow).
enum TurnEvent {
    Cancel,
    Frame(Option<RenderBlock>),
    Done(Result<runtime::TurnSummary, String>),
}

/// Drain turn frames produced after the turn future has resolved. Spectator
/// fan-out continues after a helm failure, but only the first helm write is
/// attempted so the caller cannot retain its session lock through retries. The
/// caller passes `false` after an earlier live-frame failure to suppress all
/// drain-time helm writes while still broadcasting the tail.
async fn drain_completed_turn_frames(
    block_rx: &mut mpsc::Receiver<RenderBlock>,
    session_id: &str,
    hub: &PairHub,
    writer: &ConnWriter,
    write_helm: bool,
) -> std::io::Result<()> {
    let mut first_error = None;
    while let Some(block) = block_rx.recv().await {
        hub.broadcast(session_id, &block);
        if write_helm && first_error.is_none() {
            if let Err(error) = write_frame(writer, &block).await {
                first_error = Some(error);
                block_rx.close();
            }
        }
    }
    first_error.map_or(Ok(()), Err)
}

/// `session.run_turn` → drive a turn, streaming each [`RenderBlock`] as a render
/// frame, then a terminal response carrying the turn summary.
///
/// F4: the turn is cancellable. When the client supplies a `turn_id`, a cancel
/// hook is registered so a `session.cancel_turn` (on a second connection) closes
/// the render channel — the turn observes `is_closed` and unwinds cooperatively,
/// reusing the exact local-REPL Ctrl+C contract — and the turn answers
/// [`CODE_CANCELLED`].
///
/// F2: `permission` carries the shared responder map + id space; the turn's
/// socket prompter forwards each permission gate to the client over the stream
/// and awaits a `permission.respond` resolved through this same map.
///
/// Track 5: the connection becomes the session's **helm** for the turn. Each
/// frame is written to the helm's own stream (lossless) *and* fanned out to
/// every spectator via [`PairHub::broadcast`] (drop-and-resync), so a second
/// terminal watching the session sees the turn — and any `session.steer` echo —
/// live. A second `run_turn` while a turn is in flight no longer blocks on the
/// session lock; it is refused with [`CODE_HELM_HELD`].
#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
async fn dispatch_run_turn(
    request: &RpcRequest,
    sessions: &SessionMap,
    cancels: &CancelMap,
    permission: &SocketPrompterConfig,
    hub: &PairHub,
    conn_id: u64,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let params: RunTurnParams = match serde_json::from_value(request.params.clone()) {
        Ok(params) => params,
        Err(error) => {
            return write_response(
                writer,
                RpcResponse::err(
                    request.id,
                    CODE_INVALID_PARAMS,
                    format!("invalid params: {error}"),
                ),
            )
            .await;
        }
    };
    let Some(handle) = lookup(sessions, &params.id) else {
        return write_response(
            writer,
            RpcResponse::err(
                request.id,
                CODE_NO_SUCH_SESSION,
                format!("no such session: {}", params.id),
            ),
        )
        .await;
    };

    // Turn-scoped helm: acquire the session without blocking. A held lock means
    // another connection owns the in-flight turn — refuse explicitly (§P2) so
    // the caller can spectate instead of hanging forever on the lock.
    let Ok(mut guard) = handle.try_lock() else {
        return write_response(
            writer,
            RpcResponse::err(
                request.id,
                CODE_HELM_HELD,
                format!("session {} is already running a turn (helm held)", params.id),
            ),
        )
        .await;
    };
    // A close holds this same guard while it removes the map entry and PairHub
    // channel. Once we own it, identity must still be current before this old
    // Arc can begin a turn; otherwise close won the race.
    if !session_is_current(sessions, &params.id, &handle) {
        return write_response(
            writer,
            RpcResponse::err(
                request.id,
                CODE_NO_SUCH_SESSION,
                format!("no such session: {}", params.id),
            ),
        )
        .await;
    }
    // Clone the model up front so the terminal response can read it without
    // borrowing `guard` while the pinned turn future still holds `&mut guard`.
    let model = guard.model.clone();
    let (block_tx, mut block_rx) = mpsc::channel::<RenderBlock>(64);

    // F4: register a cancel hook keyed by `(session_id, turn_id)`. The sender
    // lives for the whole turn — moved into the registry when registered,
    // retained locally otherwise — so `cancel_rx` fires only on an actual
    // `session.cancel_turn`, never on a premature drop. Retaining the sender in
    // the `None` case is load-bearing: a dropped oneshot `Sender` resolves the
    // `Receiver` with `RecvError`, and the `select!` below would then read that
    // as an explicit cancellation; holding `_retained_tx` keeps `cancel_rx`
    // pending until a real `cancel_turn` fires it.
    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
    let _retained_tx = match params.turn_id {
        Some(turn_id) => {
            cancels
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert((params.id.clone(), turn_id), cancel_tx);
            None
        }
        None => Some(cancel_tx),
    };

    // Track 5: register the helm + steering handle for this turn under a turn id
    // (server-issued when the client did not supply one), snapshotting the
    // pre-turn history so a mid-turn subscriber has a coherent replay base. Then
    // fan the user's prompt out to spectators (it travels as RPC params, not a
    // frame, so watchers would otherwise never see it).
    let turn_id = params.turn_id.unwrap_or_else(pair::next_turn_id);
    let helm_label = hub.peer_label(conn_id);
    if let Some(steering) = guard.steering_handle() {
        let snapshot = project_history(guard.runtime.session());
        hub.begin_turn(&params.id, turn_id, conn_id, helm_label, steering, snapshot);
    }
    hub.broadcast(
        &params.id,
        &RenderBlock::UserMessage {
            id: BlockIdGen::default().next(),
            text: params.input.clone(),
        },
    );

    // Background-agent completions that landed since the last turn are folded
    // into this turn's input: serve has no idle REPL pump to re-inject them as
    // their own turns, so without this sweep a detached agent's result would
    // sit in the completion store unseen forever. Spectators still see the
    // user's text as typed — the folded notices are model-facing context.
    let turn_input = {
        let completions = tools::drain_background_completions_for_session(&guard.session.id);
        tools::fold_background_completions_into_input(&completions, &params.input)
    };

    let turn_abort = runtime::HookAbortSignal::new();
    let user_cancel_requested = Arc::new(AtomicBool::new(false));
    // Inner scope: the pinned turn future borrows `&mut guard` for its whole
    // life, so it must drop before we read the post-turn history off `guard`.
    let (summary_result, cancelled, helm_error) = {
        let turn = guard.run_turn_streaming_to_channel(
            &turn_input,
            block_tx,
            permission.clone(),
            turn_abort.clone(),
            Arc::clone(&user_cancel_requested),
        );
        tokio::pin!(turn);

        let mut cancelled = false;
        let mut helm_error = None;
        let summary_result = loop {
            let event = tokio::select! {
                biased;
                _ = &mut cancel_rx, if !cancelled => TurnEvent::Cancel,
                block = block_rx.recv(), if !cancelled => TurnEvent::Frame(block),
                result = &mut turn => TurnEvent::Done(result),
            };
            match event {
                // Explicit RPC cancellation raises the runtime abort signal so
                // the turn records a typed user-cancel origin before resolving.
                TurnEvent::Cancel => {
                    user_cancel_requested.store(true, Ordering::SeqCst);
                    cancelled = true;
                    turn_abort.abort();
                }
                TurnEvent::Frame(Some(block)) => {
                    // Spectators first (never blocks — `try_send`), then the
                    // helm's own lossless stream. A dead helm socket cancels.
                    hub.broadcast(&params.id, &block);
                    if let Err(error) = write_frame(writer, &block).await {
                        // Client vanished mid-turn: stop every subsequent helm
                        // write immediately, but continue spectator fan-out.
                        helm_error = Some(error);
                        cancelled = true;
                        turn_abort.abort_host();
                        block_rx.close();
                    }
                }
                TurnEvent::Frame(None) => {}
                TurnEvent::Done(result) => {
                    // Keep draining for spectators after a dead helm, but never
                    // retry a failed helm write (the helper closes `block_rx`).
                    if let Err(error) = drain_completed_turn_frames(
                        &mut block_rx,
                        &params.id,
                        hub,
                        writer,
                        helm_error.is_none(),
                    )
                    .await
                    {
                        helm_error = Some(error);
                        cancelled = true;
                    }
                    break result;
                }
            }
        };
        (summary_result, cancelled, helm_error)
    };

    // Unregister (idempotent — `cancel_turn` may have already removed it).
    if let Some(turn_id) = params.turn_id {
        cancels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&(params.id.clone(), turn_id));
    }
    // Release the helm and refresh the cached snapshot with the completed turn.
    let post_history = project_history(guard.runtime.session());
    hub.end_turn(&params.id, turn_id, post_history);

    // A dead helm has already caused cancellation and spectator drain. Return
    // its first error after cleanup rather than attempting a terminal response.
    if let Some(error) = helm_error {
        return Err(error);
    }

    let response = match summary_result {
        Ok(summary) => {
            let mut result = summary_json(&summary, &model);
            result["next_seq"] = serde_json::json!(hub.next_seq(&params.id));
            RpcResponse::ok(request.id, result)
        }
        Err(_) if cancelled => {
            RpcResponse::err(request.id, CODE_CANCELLED, "turn cancelled".to_string())
        }
        Err(error) => RpcResponse::err(request.id, CODE_INTERNAL, format!("turn failed: {error}")),
    };
    write_response(writer, response).await
}

/// `session.subscribe` → register this connection as a spectator and return the
/// atomic `{ history, next_seq, helm }` hydration base (track 5 §2.3). Frames
/// then push to this connection's socket in the background until it unsubscribes
/// or disconnects. Read capability suffices (spectating is read-only).
async fn dispatch_subscribe(
    request: &RpcRequest,
    sessions: &SessionMap,
    hub: &PairHub,
    conn_id: u64,
    writer: &ConnWriter,
    writer_done: &mut oneshot::Receiver<()>,
) -> std::io::Result<()> {
    dispatch_subscribe_with_timeout(
        request,
        sessions,
        hub,
        conn_id,
        writer,
        writer_done,
        OUTBOUND_SEND_TIMEOUT,
    )
    .await
}

/// Testable subscribe implementation: a successful reserve is the sole path
/// that may call `subscribe_with_permit`, preserving ACK-before-activation.
#[allow(clippy::too_many_lines)]
async fn dispatch_subscribe_with_timeout(
    request: &RpcRequest,
    sessions: &SessionMap,
    hub: &PairHub,
    conn_id: u64,
    writer: &ConnWriter,
    writer_done: &mut oneshot::Receiver<()>,
    reserve_timeout: std::time::Duration,
) -> std::io::Result<()> {
    let Ok(params) = parse_params::<SubscribeParams>(request, writer).await else {
        return Ok(());
    };
    let Some(handle) = lookup(sessions, &params.id) else {
        return write_response(
            writer,
            RpcResponse::err(request.id, CODE_NO_SUCH_SESSION, format!("no such session: {}", params.id)),
        )
        .await;
    };

    // Reserve before LiveCli/PairHub locking. `subscribe_with_permit` consumes
    // it under the PairHub lock, queues the ACK, then registers the slot, so a
    // fan-out frame can never overtake the successful response.
    let out_tx = writer.sender();
    let permit = writer.reserve_with_timeout(reserve_timeout).await?;
    let outcome = if params.boundary || params.resync_v2 {
        // v2 always uses an authoritative boundary. Keep this guard through
        // PairHub activation: no run-turn broadcast can slip between snapshot
        // capture and subscriber registration.
        let guard = tokio::select! {
            guard = handle.lock() => guard,
            _ = &mut *writer_done => {
                drop(permit);
                return Err(std::io::Error::new(
                    std::io::ErrorKind::BrokenPipe,
                    "connection writer exited while subscribing",
                ));
            }
        };
        if !session_is_current(sessions, &params.id, &handle) {
            drop(permit);
            return write_response(
                writer,
                RpcResponse::err(request.id, CODE_NO_SUCH_SESSION, format!("no such session: {}", params.id)),
            )
            .await;
        }
        hub.subscribe_with_permit(
            out_tx,
            permit,
            &params.id,
            conn_id,
            Some(project_history(guard.runtime.session())),
            request.id,
            params.resync_v2,
        )
    } else if let Ok(guard) = handle.try_lock() {
        // Holding the LiveCli guard makes this check linear with close.
        if !session_is_current(sessions, &params.id, &handle) {
            drop(permit);
            return write_response(
                writer,
                RpcResponse::err(request.id, CODE_NO_SUCH_SESSION, format!("no such session: {}", params.id)),
            )
            .await;
        }
        hub.subscribe_with_permit(
            out_tx,
            permit,
            &params.id,
            conn_id,
            Some(project_history(guard.runtime.session())),
            request.id,
            false,
        )
    } else {
        // Preserve the old nonblocking cached subscription path. The session
        // map guard covers the synchronous `PairHub` activation: close either
        // removes this exact Arc first or follows it and cleans the new slot.
        // No guard reaches the error response await below.
        let mut reserved = Some(permit);
        let outcome = {
            let map = sessions.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            session_handle_is_current(&map, &params.id, &handle).then(|| {
                hub.subscribe_with_permit(
                    out_tx,
                    reserved.take().expect("reserved subscribe permit"),
                    &params.id,
                    conn_id,
                    None,
                    request.id,
                    false,
                )
            })
        };
        let Some(outcome) = outcome else {
            drop(reserved);
            return write_response(
                writer,
                RpcResponse::err(request.id, CODE_NO_SUCH_SESSION, format!("no such session: {}", params.id)),
            )
            .await;
        };
        outcome
    };
    match outcome {
        Ok(_) => {
            println!("[serve] {} · {} viewer(s) watching", params.id, hub.viewer_count(&params.id));
            Ok(()) // response was atomically queued by PairHub.
        }
        Err(SubscribeError::SubscriptionIdExhausted) => {
            write_response(
                writer,
                RpcResponse::err(
                    request.id,
                    CODE_INTERNAL,
                    "subscription identity space exhausted".to_string(),
                ),
            )
            .await
        }
    }
}

/// `session.unsubscribe` → drop this connection's spectator slot for a session.
/// Idempotent (unknown/never-subscribed answers `unsubscribed: true`).
async fn dispatch_unsubscribe(
    request: &RpcRequest,
    hub: &PairHub,
    conn_id: u64,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let Ok(params) = parse_params::<SessionIdParams>(request, writer).await else {
        return Ok(());
    };
    hub.unsubscribe(&params.id, conn_id);
    println!(
        "[serve] {} · {} viewer(s) watching",
        params.id,
        hub.viewer_count(&params.id)
    );
    write_response(
        writer,
        RpcResponse::ok(
            request.id,
            serde_json::json!({ "id": params.id, "unsubscribed": true }),
        ),
    )
    .await
}

/// `session.steer` → push a mid-turn steering message onto the in-flight turn's
/// steering queue (track 5 §2.4). Full capability is already enforced by the
/// gate (a spectator's Read token cannot reach here); this additionally requires
/// an in-flight turn to steer, answering [`CODE_STEER_DENIED`] otherwise. The
/// turn drains the queue at its next tool-result boundary and emits the
/// `⤷ steering:` echo, which fans out to every spectator.
async fn dispatch_steer(
    request: &RpcRequest,
    hub: &PairHub,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let Ok(params) = parse_params::<SteerParams>(request, writer).await else {
        return Ok(());
    };
    let response = match hub.steering_for(&params.id, params.turn_id) {
        SteerAuth::Allowed(queue) => {
            if let Ok(mut pending) = queue.lock() {
                pending.push(params.text);
            }
            RpcResponse::ok(request.id, serde_json::json!({ "steered": true }))
        }
        SteerAuth::NoActiveTurn => RpcResponse::err(
            request.id,
            CODE_STEER_DENIED,
            format!("no in-flight turn to steer on session {}", params.id),
        ),
        SteerAuth::TurnMismatch => RpcResponse::err(
            request.id,
            CODE_STEER_DENIED,
            "turn_id does not match the in-flight turn".to_string(),
        ),
    };
    write_response(writer, response).await
}

/// `session.roster` → the peers connected to the server and the current helm of
/// a session (track 5 §2.6). Read capability suffices.
async fn dispatch_roster(
    request: &RpcRequest,
    hub: &PairHub,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let Ok(params) = parse_params::<SessionIdParams>(request, writer).await else {
        return Ok(());
    };
    write_response(writer, RpcResponse::ok(request.id, hub.roster(&params.id))).await
}

/// `session.run_turn_detached` → start a background turn and return immediately
/// with a server job id. The spawned task owns the session lock for the turn;
/// the dispatcher never waits on that lock, so callers can enqueue a detached
/// job even when another client is connected.
#[allow(clippy::too_many_arguments)]
async fn dispatch_run_turn_detached(
    request: &RpcRequest,
    sessions: &SessionMap,
    cancels: &CancelMap,
    jobs: &JobMap,
    permission: &SocketPrompterConfig,
    hub: &PairHub,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let Ok(params) = parse_params::<RunTurnDetachedParams>(request, writer).await else {
        return Ok(());
    };
    let Ok(handle) = require_session(request, sessions, &params.id, writer).await else {
        return Ok(());
    };

    let job_id = next_job_id();
    insert_job(
        jobs,
        job_id,
        JobHandle::running(params.id.clone(), Instant::now()),
    );

    let session_id = params.id;
    let input = params.input;
    let turn_id = params.turn_id;
    let notify_url = params.notify_url;
    let cancels = Arc::clone(cancels);
    let jobs = Arc::clone(jobs);
    let sessions = Arc::clone(sessions);
    let permission = permission.clone();
    let hub = hub.clone();
    tokio::spawn(async move {
        run_detached_turn_task(
            job_id,
            session_id,
            handle,
            input,
            turn_id,
            notify_url,
            jobs,
            cancels,
            sessions,
            permission,
            hub,
        )
        .await;
    });

    write_response(
        writer,
        RpcResponse::ok(
            request.id,
            serde_json::json!({ "job_id": job_id, "status": "running" }),
        ),
    )
    .await
}

#[allow(clippy::too_many_arguments)]
async fn run_detached_turn_task(
    job_id: u64,
    session_id: String,
    handle: Arc<tokio::sync::Mutex<LiveCli>>,
    input: String,
    turn_id: Option<u64>,
    notify_url: Option<String>,
    jobs: JobMap,
    cancels: CancelMap,
    sessions: SessionMap,
    permission: SocketPrompterConfig,
    hub: PairHub,
) {
    let payload = {
        let mut guard = handle.lock().await;
        // A detached job can be queued just before close. The close path holds
        // this guard while removing its exact Arc, so checking after acquisition
        // linearizes the task: it either starts before close or terminates as a
        // no-such-session job without recreating PairHub state.
        if session_is_current(&sessions, &session_id, &handle) {
            run_detached_turn_with_guard(
                job_id,
                &session_id,
                &mut guard,
                input,
                turn_id,
                &jobs,
                &cancels,
                permission,
                &hub,
            )
            .await
        } else {
            finish_job_error(
                &jobs,
                job_id,
                JobStatus::Error,
                format!("no such session: {session_id}"),
            )
        }
    };

    if let Some(url) = notify_url {
        let _ = tools::notify_remote(
            &url,
            serde_json::json!({
                "event": "session.run_turn_detached.completed",
                "job_id": job_id,
                "session_id": session_id,
                "payload": payload,
            }),
        )
        .await;
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_detached_turn_with_guard(
    job_id: u64,
    session_id: &str,
    guard: &mut LiveCli,
    input: String,
    turn_id: Option<u64>,
    jobs: &JobMap,
    cancels: &CancelMap,
    permission: SocketPrompterConfig,
    hub: &PairHub,
) -> serde_json::Value {
    let model = guard.model.clone();
    let (block_tx, mut block_rx) = mpsc::channel::<RenderBlock>(64);

    let (cancel_tx, mut cancel_rx) = tokio::sync::oneshot::channel::<()>();
    // Retaining the sender in the `None` case is load-bearing (see
    // `dispatch_run_turn`): a dropped oneshot `Sender` resolves the
    // `Receiver` with `RecvError`, which the `select!` would misread as a
    // cancel. Holding `_retained_tx` keeps `cancel_rx` pending until a real
    // `cancel_turn` fires it — a detached turn is cancelled the same way.
    let _retained_tx = match turn_id {
        Some(turn_id) => {
            cancels
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert((session_id.to_string(), turn_id), cancel_tx);
            None
        }
        None => Some(cancel_tx),
    };

    // Track 5: a detached turn is still a session turn — register it as the
    // helm and fan its frames out so a spectator watching the session sees a
    // background turn, not just an attached one. The helm has no live socket
    // (it detached), so a `conn_id` of 0 marks the roster's helm as absent.
    let helm_turn_id = turn_id.unwrap_or_else(pair::next_turn_id);
    if let Some(steering) = guard.steering_handle() {
        let snapshot = project_history(guard.runtime.session());
        hub.begin_turn(session_id, helm_turn_id, 0, "detached".to_string(), steering, snapshot);
    }
    hub.broadcast(
        session_id,
        &RenderBlock::UserMessage {
            id: BlockIdGen::default().next(),
            text: input.clone(),
        },
    );

    let turn_abort = runtime::HookAbortSignal::new();
    let user_cancel_requested = Arc::new(AtomicBool::new(false));
    // Inner scope: the pinned turn future borrows `&mut guard`; it must drop
    // before we read the post-turn history off `guard`.
    let (summary_result, cancelled) = {
        let turn = guard.run_turn_streaming_to_channel(
            &input,
            block_tx,
            permission,
            turn_abort.clone(),
            Arc::clone(&user_cancel_requested),
        );
        tokio::pin!(turn);

        let mut cancelled = false;
        let summary_result = loop {
            let event = tokio::select! {
                biased;
                _ = &mut cancel_rx, if !cancelled => TurnEvent::Cancel,
                block = block_rx.recv(), if !cancelled => TurnEvent::Frame(block),
                result = &mut turn => TurnEvent::Done(result),
            };
            match event {
                TurnEvent::Cancel => {
                    user_cancel_requested.store(true, Ordering::SeqCst);
                    cancelled = true;
                    turn_abort.abort();
                }
                TurnEvent::Frame(Some(block)) => {
                    hub.broadcast(session_id, &block);
                    if let Some(frame) = render_block_json(&block) {
                        record_job_frame(jobs, job_id, frame);
                    }
                }
                TurnEvent::Frame(None) => {}
                TurnEvent::Done(result) => {
                    while let Some(block) = block_rx.recv().await {
                        hub.broadcast(session_id, &block);
                        if let Some(frame) = render_block_json(&block) {
                            record_job_frame(jobs, job_id, frame);
                        }
                    }
                    break result;
                }
            }
        };
        (summary_result, cancelled)
    };

    if let Some(turn_id) = turn_id {
        cancels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&(session_id.to_string(), turn_id));
    }
    let post_history = project_history(guard.runtime.session());
    hub.end_turn(session_id, helm_turn_id, post_history);

    match summary_result {
        Ok(summary) => finish_job_success(jobs, job_id, summary_json(&summary, &model)),
        Err(_) if cancelled => finish_job_error(
            jobs,
            job_id,
            JobStatus::Cancelled,
            "turn cancelled".to_string(),
        ),
        Err(error) => finish_job_error(jobs, job_id, JobStatus::Error, error),
    }
}

/// `session.job_status` → inspect a detached job without removing it.
async fn dispatch_job_status(
    request: &RpcRequest,
    jobs: &JobMap,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let Ok(params) = parse_params::<JobIdParams>(request, writer).await else {
        return Ok(());
    };
    write_response(
        writer,
        RpcResponse::ok(request.id, job_status_json(jobs, params.job_id)),
    )
    .await
}

/// `session.job_result` → read a terminal detached job and remove it. A running
/// job is reported but retained so a later call can still collect the result.
async fn dispatch_job_result(
    request: &RpcRequest,
    jobs: &JobMap,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let Ok(params) = parse_params::<JobIdParams>(request, writer).await else {
        return Ok(());
    };
    write_response(
        writer,
        RpcResponse::ok(request.id, job_result_json(jobs, params.job_id)),
    )
    .await
}

fn next_job_id() -> u64 {
    NEXT_JOB_ID.fetch_add(1, Ordering::Relaxed)
}

fn render_block_json(block: &RenderBlock) -> Option<serde_json::Value> {
    use zo_cli::sinks::{NdjsonSink, Sink};
    let mut buffer = Vec::new();
    {
        let mut sink = NdjsonSink::new(&mut buffer);
        sink.emit(block).ok()?;
    }
    serde_json::from_slice(&buffer).ok()
}

fn insert_job(jobs: &JobMap, job_id: u64, job: JobHandle) {
    let mut map = jobs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    prune_jobs_locked(&mut map, Instant::now());
    map.insert(job_id, job);
    prune_jobs_locked(&mut map, Instant::now());
}

fn record_job_frame(jobs: &JobMap, job_id: u64, frame: serde_json::Value) {
    if let Some(job) = jobs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get_mut(&job_id)
    {
        job.push_frame(frame);
    }
}

fn finish_job_success(jobs: &JobMap, job_id: u64, result: serde_json::Value) -> serde_json::Value {
    finish_job(jobs, job_id, |job, now| job.finish_success(result, now))
}

fn finish_job_error(
    jobs: &JobMap,
    job_id: u64,
    status: JobStatus,
    error: String,
) -> serde_json::Value {
    finish_job(jobs, job_id, |job, now| {
        job.finish_error(status, error, now);
    })
}

fn finish_job(
    jobs: &JobMap,
    job_id: u64,
    finish: impl FnOnce(&mut JobHandle, Instant),
) -> serde_json::Value {
    let mut map = jobs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let now = Instant::now();
    let payload = match map.get_mut(&job_id) {
        Some(job) => {
            finish(job, now);
            job.result_json(job_id)
        }
        None => missing_job_json(job_id),
    };
    prune_jobs_locked(&mut map, now);
    payload
}

fn job_status_json(jobs: &JobMap, job_id: u64) -> serde_json::Value {
    let mut map = jobs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    prune_jobs_locked(&mut map, Instant::now());
    map.get(&job_id)
        .map_or_else(|| missing_job_json(job_id), |job| job.status_json(job_id))
}

fn job_result_json(jobs: &JobMap, job_id: u64) -> serde_json::Value {
    let mut map = jobs
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    prune_jobs_locked(&mut map, Instant::now());
    let Some(job) = map.get(&job_id) else {
        return missing_job_json(job_id);
    };
    if !job.status.is_terminal() {
        return job.status_json(job_id);
    }
    map.remove(&job_id)
        .map_or_else(|| missing_job_json(job_id), |job| job.result_json(job_id))
}

fn missing_job_json(job_id: u64) -> serde_json::Value {
    serde_json::json!({
        "job_id": job_id,
        "status": "missing",
        "done": true,
        "frame_count": 0,
    })
}

fn prune_jobs_locked(map: &mut HashMap<u64, JobHandle>, now: Instant) {
    map.retain(|_, job| !job.is_expired(now));
    while map.len() > MAX_JOBS {
        let Some(oldest_done) = map
            .iter()
            .filter_map(|(id, job)| job.completed_at.map(|completed| (*id, completed)))
            .min_by_key(|(_, completed)| *completed)
            .map(|(id, _)| id)
        else {
            break;
        };
        map.remove(&oldest_done);
    }
}

/// `session.cancel_turn` → signal an in-flight turn (registered under
/// `(session_id, turn_id)` by `session.run_turn` on another connection) to
/// cancel. Idempotent: an unknown / already-finished turn answers
/// `{ "cancelled": false }`.
///
/// The registry key is scoped by session id so two sessions that share a
/// `turn_id` never cancel each other. A client SHOULD send `session_id`; a
/// legacy client that omits it is honoured only when exactly one in-flight turn
/// carries the requested `turn_id`. If two or more sessions share it the
/// request is refused (`cancelled: false`, `"ambiguous"`) rather than
/// cancelling the wrong session's turn.
async fn dispatch_cancel_turn(
    request: &RpcRequest,
    cancels: &CancelMap,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let params: CancelTurnParams = match serde_json::from_value(request.params.clone()) {
        Ok(params) => params,
        Err(error) => {
            return write_response(
                writer,
                RpcResponse::err(
                    request.id,
                    CODE_INVALID_PARAMS,
                    format!("invalid params: {error}"),
                ),
            )
            .await;
        }
    };
    let (cancelled, message) = {
        let mut map = cancels
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(session_id) = params.session_id.as_ref() {
            // Scoped remove: cancel exactly the named session's turn.
            match map.remove(&(session_id.clone(), params.turn_id)) {
                Some(tx) => (tx.send(()).is_ok(), "cancellation signalled".to_string()),
                None => (false, "no in-flight turn with that id".to_string()),
            }
        } else {
            // Legacy path: match on `turn_id` alone, but only act when it is
            // unambiguous. Refuse a cross-session collision rather than guess.
            let matching: Vec<(String, u64)> = map
                .keys()
                .filter(|(_, turn_id)| *turn_id == params.turn_id)
                .cloned()
                .collect();
            match matching.len() {
                0 => (false, "no in-flight turn with that id".to_string()),
                1 => {
                    let key = matching.into_iter().next().unwrap();
                    match map.remove(&key) {
                        Some(tx) => (tx.send(()).is_ok(), "cancellation signalled".to_string()),
                        None => (false, "no in-flight turn with that id".to_string()),
                    }
                }
                n => (
                    false,
                    format!(
                        "ambiguous: {n} sessions have turn_id {}; pass session_id to disambiguate",
                        params.turn_id
                    ),
                ),
            }
        }
    };
    write_response(
        writer,
        RpcResponse::ok(
            request.id,
            serde_json::json!({ "cancelled": cancelled, "message": message }),
        ),
    )
    .await
}

/// `permission.respond` → resolve a forwarded permission prompt (F2). Looks up
/// the parked responder by `prompt_id` (registered by the turn's socket prompter
/// on another connection), maps the wire decision tag through the shared
/// serializer table, and fires the oneshot — unblocking the waiting turn.
/// Idempotent: an unknown / already-answered / timed-out prompt answers
/// `{ "resolved": false }`.
async fn dispatch_permission_respond(
    request: &RpcRequest,
    permission: &SocketPrompterConfig,
    writer: &ConnWriter,
) -> std::io::Result<()> {
    let params: PermissionRespondParams = match serde_json::from_value(request.params.clone()) {
        Ok(params) => params,
        Err(error) => {
            return write_response(
                writer,
                RpcResponse::err(
                    request.id,
                    CODE_INVALID_PARAMS,
                    format!("invalid params: {error}"),
                ),
            )
            .await;
        }
    };
    let decision = permission_decision_from_tag(&params.decision);
    let responder = permission
        .responders
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .remove(&params.prompt_id);
    let resolved = match responder {
        Some(tx) => tx.send(decision).is_ok(),
        None => false,
    };
    write_response(
        writer,
        RpcResponse::ok(request.id, serde_json::json!({ "resolved": resolved })),
    )
    .await
}

/// Clone the session handle out of the map (dropping the map lock immediately).
fn lookup(sessions: &SessionMap, id: &str) -> Option<Arc<tokio::sync::Mutex<LiveCli>>> {
    sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(id)
        .cloned()
}

/// Whether `handle` is still the exact live session registered under `id`.
/// Call this while holding that handle's `LiveCli` guard when linearizing a
/// lifecycle operation against `session.close`.
fn session_is_current(
    sessions: &SessionMap,
    id: &str,
    handle: &Arc<tokio::sync::Mutex<LiveCli>>,
) -> bool {
    let map = sessions
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    session_handle_is_current(&map, id, handle)
}

/// Generic map-lock identity predicate. The legacy nonblocking subscribe path
/// keeps its session map guard across synchronous `PairHub` activation, so close
/// cannot remove this Arc between the identity test and registration.
fn session_handle_is_current<T>(map: &HashMap<String, Arc<T>>, id: &str, handle: &Arc<T>) -> bool {
    map.get(id).is_some_and(|current| Arc::ptr_eq(current, handle))
}

/// Project a persisted [`Session`](runtime::session::Session) into the flat
/// `role + text` history the load response carries. Tool calls/results collapse
/// to a single annotated line so the client can show *that* a tool ran without
/// the server shipping full payloads.
fn project_history(session: &runtime::session::Session) -> Vec<HistoryEntry> {
    use runtime::session::{ContentBlock, MessageRole};
    session
        .messages
        .iter()
        .map(|message| {
            let role = match message.role {
                MessageRole::System => "system",
                MessageRole::User => "user",
                MessageRole::Assistant => "assistant",
                MessageRole::Tool => "tool",
            };
            let text = message
                .blocks
                .iter()
                .map(|block| match block {
                    ContentBlock::Text { text } => text.clone(),
                    ContentBlock::ToolUse { name, .. } => format!("⚙ {name}"),
                    ContentBlock::ToolResult {
                        tool_name,
                        is_error,
                        ..
                    } => {
                        if *is_error {
                            format!("✗ {tool_name}")
                        } else {
                            format!("✓ {tool_name}")
                        }
                    }
                    ContentBlock::Image { media_type, .. } => format!("[image {media_type}]"),
                    ContentBlock::Thinking { .. } => "[thinking]".to_string(),
                    ContentBlock::RedactedThinking { .. } => "[redacted thinking]".to_string(),
                })
                .collect::<Vec<_>>()
                .join("\n");
            HistoryEntry {
                role: role.to_string(),
                text,
            }
        })
        .collect()
}

/// Build the terminal `session.run_turn` result body from the turn summary.
fn summary_json(summary: &runtime::TurnSummary, model: &str) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "iterations": summary.iterations,
        "usage": {
            "input_tokens": summary.usage.input_tokens,
            "output_tokens": summary.usage.output_tokens,
            "cache_read_input_tokens": summary.usage.cache_read_input_tokens,
            "cache_creation_input_tokens": summary.usage.cache_creation_input_tokens,
        },
    })
}

/// Serialize one [`RenderBlock`] as a render frame and hand it to the
/// connection's write funnel. Reuses the canonical `SerializableRenderBlock`
/// serialization (via [`NdjsonSink`](zo_cli::sinks::NdjsonSink)) so a
/// frame is byte-identical to a `zo -p --output-format stream-json` line.
async fn write_frame(writer: &ConnWriter, block: &RenderBlock) -> std::io::Result<()> {
    use zo_cli::sinks::{NdjsonSink, Sink};
    let mut buffer: Vec<u8> = Vec::new();
    {
        let mut sink = NdjsonSink::new(&mut buffer);
        // Writing into a `Vec<u8>` is infallible; the only error path is a
        // serialization bug, which we surface as an empty frame rather than
        // tearing down the connection.
        let _ = sink.emit(block);
    }
    // `NdjsonSink` already terminates each frame with `\n`.
    writer.send(Arc::from(String::from_utf8_lossy(&buffer).into_owned())).await
}

/// Serialize a JSON-RPC response as one `\n`-terminated line and hand it to the
/// connection's write funnel.
async fn write_response(writer: &ConnWriter, response: RpcResponse) -> std::io::Result<()> {
    let mut line = serde_json::to_string(&response).unwrap_or_else(|_| {
        r#"{"jsonrpc":"2.0","id":0,"error":{"code":-32603,"message":"serialize"}}"#.to_string()
    });
    line.push('\n');
    writer.send(Arc::from(line)).await
}

/// A stuck socket writer must not let a helm retain the session lock forever.
const OUTBOUND_SEND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// The single per-connection outbound funnel. Every socket write — RPC
/// responses, the helm's own turn frames, fanned-out spectator frames, and
/// roster/resync control frames — is a pre-serialized `\n`-terminated line sent
/// here and drained, in order, by one writer task ([`spawn_conn_writer`]). This
/// is what lets a background subscription push frames onto the same connection a
/// dispatch loop is answering requests on without interleaving bytes.
#[derive(Clone)]
pub(crate) struct ConnWriter {
    tx: mpsc::Sender<Arc<str>>,
    send_timeout: std::time::Duration,
}

impl ConnWriter {
    fn new(tx: mpsc::Sender<Arc<str>>) -> Self {
        Self {
            tx,
            send_timeout: OUTBOUND_SEND_TIMEOUT,
        }
    }

    #[cfg(test)]
    fn with_timeout(tx: mpsc::Sender<Arc<str>>, send_timeout: std::time::Duration) -> Self {
        Self { tx, send_timeout }
    }

    /// Send one already-serialized line, mapping a closed funnel (writer task
    /// gone — the socket died) to a `BrokenPipe` error so the dispatch loop
    /// unwinds exactly as a failed `write_all` did before.
    async fn send(&self, line: Arc<str>) -> std::io::Result<()> {
        self.send_with_timeout(line, self.send_timeout).await
    }

    async fn send_with_timeout(&self, line: Arc<str>, timeout: std::time::Duration) -> std::io::Result<()> {
        match tokio::time::timeout(timeout, self.tx.send(line)).await {
            Ok(Ok(())) => Ok(()),
            Ok(Err(_)) => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "connection write channel closed",
            )),
            Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "connection write channel stalled",
            )),
        }
    }

    /// Reserve outbound capacity before an operation that must queue its
    /// response before activating a fan-out subscriber. A full or closed funnel
    /// makes the connection unusable, so both cases become `BrokenPipe` and the
    /// read loop tears it down instead of waiting forever.
    async fn reserve_with_timeout(
        &self,
        duration: std::time::Duration,
    ) -> std::io::Result<mpsc::Permit<'_, Arc<str>>> {
        match tokio::time::timeout(duration, self.tx.reserve()).await {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(_)) | Err(_) => Err(std::io::Error::new(
                std::io::ErrorKind::BrokenPipe,
                "connection write channel unavailable",
            )),
        }
    }

    /// A clone of the outbound sender, handed to [`pair::PairHub::subscribe`] so
    /// fanned frames land on this same ordered stream.
    fn sender(&self) -> mpsc::Sender<Arc<str>> {
        self.tx.clone()
    }
}

/// Drain queued lines to one writer with a bounded socket write. Kept generic
/// so the timeout behavior is directly testable with an in-memory writer.
async fn drain_conn_writer<W>(
    mut socket: W,
    mut out_rx: mpsc::Receiver<Arc<str>>,
    write_timeout: std::time::Duration,
) -> std::io::Result<()>
where
    W: AsyncWrite + Unpin,
{
    while let Some(line) = out_rx.recv().await {
        match tokio::time::timeout(write_timeout, socket.write_all(line.as_bytes())).await {
            Ok(Ok(())) => {},
            Ok(Err(error)) => return Err(error),
            Err(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::TimedOut,
                    "connection socket write stalled",
                ));
            }
        }
    }
    match tokio::time::timeout(write_timeout, socket.flush()).await {
        Ok(result) => result,
        Err(_) => Err(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "connection socket flush stalled",
        )),
    }
}

/// Drain a connection's outbound funnel to its socket write half, in order,
/// until the funnel closes (all `ConnWriter` clones dropped) or the socket
/// errors. The completion signal wakes the paired read loop immediately so a
/// dead writer cannot leave it blocked in `next_line`.
fn spawn_conn_writer(
    socket: OwnedWriteHalf,
    out_rx: mpsc::Receiver<Arc<str>>,
    done: oneshot::Sender<()>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let _ = drain_conn_writer(socket, out_rx, OUTBOUND_SEND_TIMEOUT).await;
        let _ = done.send(());
    })
}

#[cfg(test)]
mod tests;
