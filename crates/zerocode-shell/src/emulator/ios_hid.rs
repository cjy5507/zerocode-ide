//! macOS side of the iOS Simulator HID helper protocol.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, OnceLock, mpsc};
use std::time::{Duration, Instant};

use base64::Engine;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const HELPER_BYTES: &[u8] = include_bytes!(env!("ZEROCODE_IOS_HID_HELPER"));
const REQUEST_TIMEOUT: Duration = Duration::from_secs(4);
const PASTEBOARD_TIMEOUT: Duration = Duration::from_secs(4);
const MAX_DIAGNOSTIC_BYTES: usize = 4 * 1024;

/// The fixed head of one pushed picture: byte count, surface generation, and
/// the size it was encoded at, each a big-endian `u32`.
///
/// Big-endian to match the only other binary wire this crate owns (the raw
/// Tauri channel's sequence prefix in `mod.rs`), so there is one answer in this
/// codebase to "which way round do the bytes go" rather than two.
const FRAME_HEADER_BYTES: usize = 4 * 4;

/// The largest single pushed picture that is believed rather than treated as a
/// desynchronised pipe. A full-resolution iPhone framebuffer encodes to a few
/// hundred kilobytes; anything past this is not a picture.
const MAX_PUSH_FRAME_BYTES: usize = 8 * 1024 * 1024;

/// How long the frame socket waits for the helper to dial back.
///
/// The helper loads the active Xcode's SimulatorKit and opens a CoreSimulator
/// service context before it can connect, which is the same cold start the
/// capability thread already spends 247-495ms on — and much longer on a
/// machine that is paging Xcode's frameworks in for the first time.
const FRAME_SOCKET_ACCEPT_TIMEOUT: Duration = Duration::from_secs(20);

/// How often the accept loop looks while it waits. Small enough that a helper
/// that connects immediately is not made to wait for a tick.
const FRAME_SOCKET_ACCEPT_POLL: Duration = Duration::from_millis(5);

const FRAME_SOCKET_READ_CHUNK: usize = 64 * 1024;

#[derive(Debug, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub(super) enum InputRequest {
    /// One phase of a live touch — the mouse streamed as the finger it is.
    /// `phase` is "begin", "move" or "end"; the window keeps the pacing, so
    /// the device animates WITH the hand instead of replaying it afterwards.
    Touch {
        phase: String,
        x: f64,
        y: f64,
    },
    Tap {
        x: f64,
        y: f64,
    },
    Swipe {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        duration_ms: u32,
    },
    MultiTouch {
        x1: f64,
        y1: f64,
        x2: f64,
        y2: f64,
        x3: f64,
        y3: f64,
        x4: f64,
        y4: f64,
        duration_ms: u32,
    },
    Text {
        text: String,
    },
    Button {
        name: String,
    },
    Rotate {
        rotation: u32,
    },
    Ax,
    Ping,
    Paste,
    /// One picture of the device's own framebuffer, asked for and waited on.
    ///
    /// `seed` is the surface generation the caller already has, and the helper
    /// answers nothing at all when it has not moved — measured at 35µs, which
    /// is what makes a still pane free rather than merely cheap.
    ///
    /// The fallback road now that [`InputRequest::Stream`] exists: it costs a
    /// whole request/response round trip through this process's ONE request
    /// lane, so a pane that used it for every frame was making its own touches
    /// queue behind its own pictures.
    Frame {
        seed: Option<u32>,
        long_edge: u32,
        quality: f64,
    },
    /// Start pushing pictures down the frame socket as fast as the framebuffer
    /// changes, up to `max_fps`.
    ///
    /// The road the pane actually wants. Asking costs one message; after it,
    /// pictures arrive on their own socket without touching the request lane at
    /// all, so a picture never waits behind a touch and a touch never waits
    /// behind a picture. Sent again with different numbers to re-negotiate.
    Stream {
        long_edge: u32,
        quality: f64,
        max_fps: u32,
    },
    /// Stop pushing. Said when the pane is hidden or paused, so the encoder
    /// idles instead of burning a core on pictures nobody is looking at.
    StreamStop,
}

/// A picture the helper encoded, with the generation and size it was taken at.
pub(super) struct HelperFrame {
    pub(super) bytes: Vec<u8>,
    pub(super) seed: u32,
    pub(super) width: u32,
    pub(super) height: u32,
}

#[derive(Serialize)]
struct WireRequest<'a> {
    id: u64,
    #[serde(flatten)]
    request: &'a InputRequest,
}

#[derive(Deserialize)]
struct WireResponse {
    id: u64,
    ok: bool,
    error: Option<String>,
    data: Option<String>,
    /// Only a frame answers this, and it answers it even when `data` is empty —
    /// that pair is how "nothing changed" is told apart from "nothing to say".
    #[serde(default)]
    seed: Option<u32>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
}

/// The newest picture the helper has pushed, and nothing older.
///
/// One slot rather than a queue, and that is the whole design: a mirror is only
/// ever worth its LATEST frame. If the renderer falls behind, the right answer
/// is for the pane to skip ahead to now, not to work through a backlog of
/// pictures of the past — a queue here would turn a momentary hitch into
/// permanent lag that never drains.
struct FrameBus {
    latest: Mutex<Option<HelperFrame>>,
    arrived: Condvar,
    /// Set when the socket behind this bus is gone. A waiter must be able to
    /// tell "the screen has not moved" from "nothing will ever arrive again",
    /// because the pump answers those two with different roads.
    closed: AtomicBool,
}

impl FrameBus {
    fn new() -> Self {
        Self {
            latest: Mutex::new(None),
            arrived: Condvar::new(),
            closed: AtomicBool::new(false),
        }
    }

    fn put(&self, frame: HelperFrame) {
        *held(&self.latest) = Some(frame);
        self.arrived.notify_all();
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        self.arrived.notify_all();
    }

    fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// The newest picture, waiting up to `timeout` for one to arrive.
    ///
    /// `Err` means the socket is gone and waiting again is pointless; `Ok(None)`
    /// means the framebuffer simply has not moved, which on a still screen is
    /// the common and correct answer.
    fn take(&self, timeout: Duration) -> Result<Option<HelperFrame>, String> {
        let mut latest = held(&self.latest);
        if let Some(frame) = latest.take() {
            return Ok(Some(frame));
        }
        if self.is_closed() {
            return Err("iOS 화면 스트림이 닫혔습니다".to_string());
        }
        let (mut latest, _) = self
            .arrived
            .wait_timeout(latest, timeout)
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(frame) = latest.take() {
            return Ok(Some(frame));
        }
        if self.is_closed() {
            return Err("iOS 화면 스트림이 닫혔습니다".to_string());
        }
        Ok(None)
    }
}

/// Cut whole pictures out of whatever the socket happened to hand over.
///
/// A stream socket knows nothing about picture boundaries — one read can carry
/// three frames, or the first nine bytes of one header — so the length prefix
/// is the only truth about where a picture ends. Consumed bytes leave the
/// buffer and a partial tail stays for the next read to complete.
///
/// Kept apart from the socket so every boundary it has to survive is a table
/// test rather than something only a booted simulator can produce.
fn drain_frames(buffer: &mut Vec<u8>) -> Result<Vec<HelperFrame>, String> {
    let mut frames = Vec::new();
    let mut consumed = 0usize;
    while buffer.len() - consumed >= FRAME_HEADER_BYTES {
        let head = &buffer[consumed..consumed + FRAME_HEADER_BYTES];
        let number = |at: usize| {
            u32::from_be_bytes([head[at], head[at + 1], head[at + 2], head[at + 3]]) as usize
        };
        let length = number(0);
        if length == 0 || length > MAX_PUSH_FRAME_BYTES {
            // Not a short read — a length this side of sane cannot become sane
            // by waiting, so the pipe is desynchronised and the caller must
            // drop it rather than spin on the same bytes forever.
            return Err(format!(
                "iOS 화면 조각의 길이가 올바르지 않습니다: {length}"
            ));
        }
        let width = number(8);
        let height = number(12);
        if width == 0 || height == 0 {
            return Err("iOS 화면 조각의 크기가 0입니다".to_string());
        }
        let body = consumed + FRAME_HEADER_BYTES;
        if buffer.len() - body < length {
            break;
        }
        frames.push(HelperFrame {
            bytes: buffer[body..body + length].to_vec(),
            seed: number(4) as u32,
            width: width as u32,
            height: height as u32,
        });
        consumed = body + length;
    }
    if consumed > 0 {
        buffer.drain(..consumed);
    }
    Ok(frames)
}

struct ProcessState {
    child: Child,
    stdin: ChildStdin,
    replies: mpsc::Receiver<String>,
    next_id: u64,
}

struct InputClient {
    state: Mutex<ProcessState>,
    diagnostic: Arc<Mutex<String>>,
    /// Where pushed pictures land. Separate from `state` on purpose: this is
    /// the whole reason a touch no longer queues behind a picture, and sharing
    /// one lock between them would put the queue straight back.
    frames: Arc<FrameBus>,
    /// The socket file to unlink when this client goes. `None` on a machine
    /// where binding failed, which is not fatal — the helper then runs with the
    /// argument list it has always had and only the pulled road works.
    socket: Option<PathBuf>,
    /// Set the moment this client kills its own helper.
    ///
    /// Every road out of `ask` that fails kills the process, and a killed
    /// helper never answers again — but the entry holding it stays in the map
    /// until the last pane releases it, so without this a later `retain` is
    /// handed the corpse and reports success over it. That pane then spends a
    /// 4-second timeout per frame on a process that exited minutes ago.
    dead: AtomicBool,
}

/// Somewhere short enough for a Unix socket to live.
///
/// `sockaddr_un.sun_path` is 104 bytes on macOS and `bind` fails outright past
/// it, so this deliberately does not go under the application's own data root —
/// that path is long, user-controlled, and would push a working machine over
/// the limit for no gain. The name carries no device identifier because the
/// helper is told it on argv, and a shorter name is a safer name here.
fn frame_socket_path() -> PathBuf {
    let id = uuid::Uuid::new_v4().simple().to_string();
    std::env::temp_dir().join(format!("zc-ios-{}.sock", &id[..12]))
}

/// Why an ask failed — and whether the helper ever had it.
enum AskFailure {
    /// The pipe was already closed: the helper had died before this request
    /// left the process. Nothing was applied, so a fresh helper may be asked
    /// the same thing.
    Unsent(String),
    /// The helper had the request — it may have acted before failing, or it
    /// refused. Not asked again.
    Failed(String),
}

impl AskFailure {
    fn into_message(self) -> String {
        match self {
            Self::Unsent(message) | Self::Failed(message) => message,
        }
    }
}

impl InputClient {
    fn start(udid: &str) -> Result<Arc<Self>, String> {
        let executable = helper_program()?;
        // Bound BEFORE the helper is spawned so there is no window in which it
        // dials a socket nobody is listening on yet.
        let socket = frame_socket_path();
        let listener = UnixListener::bind(&socket)
            .ok()
            .filter(|listener| listener.set_nonblocking(true).is_ok());
        let mut command = crate::proc::quiet_command(executable);
        command.arg(udid);
        if listener.is_some() {
            command.arg(&socket);
        } else {
            let _ = std::fs::remove_file(&socket);
        }
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| format!("iOS 입력 헬퍼를 시작할 수 없습니다: {error}"))?;
        let Some(stdin) = child.stdin.take() else {
            stop_process(&mut child);
            return Err("iOS 입력 헬퍼의 입력 문이 없습니다".to_string());
        };
        let Some(stdout) = child.stdout.take() else {
            stop_process(&mut child);
            return Err("iOS 입력 헬퍼의 응답 문이 없습니다".to_string());
        };
        let Some(stderr) = child.stderr.take() else {
            stop_process(&mut child);
            return Err("iOS 입력 헬퍼의 진단 문이 없습니다".to_string());
        };
        let (reply_tx, reply_rx) = mpsc::channel();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("ios-hid-replies-{udid}"))
            .spawn(move || {
                for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                    if reply_tx.send(line).is_err() {
                        break;
                    }
                }
            })
        {
            stop_process(&mut child);
            return Err(format!("iOS 입력 응답기를 시작할 수 없습니다: {error}"));
        }
        let diagnostic = Arc::new(Mutex::new(String::new()));
        let diagnostic_sink = diagnostic.clone();
        if let Err(error) = std::thread::Builder::new()
            .name(format!("ios-hid-diagnostics-{udid}"))
            .spawn(move || {
                let mut bytes = Vec::new();
                let _ = BufReader::new(stderr)
                    .take(MAX_DIAGNOSTIC_BYTES as u64)
                    .read_to_end(&mut bytes);
                *held(&diagnostic_sink) = String::from_utf8_lossy(&bytes).trim().to_string();
            })
        {
            stop_process(&mut child);
            return Err(format!("iOS 입력 진단기를 시작할 수 없습니다: {error}"));
        }

        let frames = Arc::new(FrameBus::new());
        let client = Arc::new(Self {
            state: Mutex::new(ProcessState {
                child,
                stdin,
                replies: reply_rx,
                next_id: 1,
            }),
            diagnostic,
            frames: frames.clone(),
            socket: listener.is_some().then(|| socket.clone()),
            dead: AtomicBool::new(false),
        });
        if let Some(listener) = listener {
            let bus = frames;
            if std::thread::Builder::new()
                .name(format!("ios-hid-frames-{udid}"))
                .spawn(move || {
                    serve_frame_socket(&listener, &bus);
                    // Whatever ended the service — a helper that never dialled,
                    // a listener that gave out, this client being put down —
                    // every waiter has to learn it, or a pane sits on a bus
                    // nothing will ever arrive at.
                    bus.close();
                })
                .is_err()
            {
                // The pulled road is still there, so this is a degradation and
                // not a failure. Saying so is what stops a waiter blocking.
                client.frames.close();
            }
        } else {
            client.frames.close();
        }
        let _ = client.request(&InputRequest::Ping)?;
        Ok(client)
    }

    fn alive(&self) -> bool {
        !self.dead.load(Ordering::Acquire)
    }

    /// Kill the helper and remember that it is gone.
    ///
    /// Always both, never only the kill — the memory is what stops the next
    /// pane being handed this client.
    fn fell_over(&self, state: &mut ProcessState) {
        stop_process(&mut state.child);
        self.dead.store(true, Ordering::Release);
    }

    /// The whole answer, because a frame needs more of it than a tap does.
    fn ask(&self, request: &InputRequest) -> Result<WireResponse, AskFailure> {
        let mut state = held(&self.state);
        let id = state.next_id;
        state.next_id = state.next_id.wrapping_add(1).max(1);
        let mut line = serde_json::to_vec(&WireRequest { id, request })
            .map_err(|error| AskFailure::Failed(error.to_string()))?;
        line.push(b'\n');
        if let Err(error) = state
            .stdin
            .write_all(&line)
            .and_then(|()| state.stdin.flush())
        {
            self.fell_over(&mut state);
            return Err(AskFailure::Unsent(format!(
                "iOS 입력 헬퍼에 보낼 수 없습니다: {error}"
            )));
        }
        let answer = match state.replies.recv_timeout(REQUEST_TIMEOUT) {
            Ok(answer) => answer,
            Err(error) => {
                self.fell_over(&mut state);
                let diagnostic = held(&self.diagnostic).clone();
                let detail = if diagnostic.is_empty() {
                    error.to_string()
                } else {
                    diagnostic
                };
                return Err(AskFailure::Failed(format!(
                    "iOS 입력 헬퍼가 응답하지 않습니다: {detail}"
                )));
            }
        };
        let response: WireResponse = match serde_json::from_str(&answer) {
            Ok(response) => response,
            Err(error) => {
                self.fell_over(&mut state);
                return Err(AskFailure::Failed(format!("잘못된 iOS 입력 응답: {error}")));
            }
        };
        if response.id != id {
            self.fell_over(&mut state);
            return Err(AskFailure::Failed(
                "iOS 입력 응답 순서가 맞지 않습니다".to_string(),
            ));
        }
        if response.ok {
            Ok(response)
        } else {
            Err(AskFailure::Failed(
                response
                    .error
                    .unwrap_or_else(|| "iOS 입력이 거절됐습니다".to_string()),
            ))
        }
    }

    fn request(&self, request: &InputRequest) -> Result<Option<String>, String> {
        self.ask(request)
            .map(|answer| answer.data)
            .map_err(AskFailure::into_message)
    }
}

impl Drop for InputClient {
    fn drop(&mut self) {
        stop_process(&mut held(&self.state).child);
        self.frames.close();
        if let Some(socket) = &self.socket {
            // The frame thread may be asleep in `accept`, waiting for the next
            // stream this helper will never open now. One knock wakes it; the
            // bus closed above is what tells it to go home.
            let _ = UnixStream::connect(socket);
            let _ = std::fs::remove_file(socket);
        }
    }
}

/// Feed the bus from the helper's connections, one after another, for as long
/// as this client lives.
///
/// Successive, not one: the helper dials this socket per STREAM rather than
/// per process — its pusher thread connects when it is told to start and
/// closes that end the moment its orders are superseded (main.swift's
/// `FramePusher.pump`, `defer { close(fd) }`). A re-negotiated size, a pane
/// that paused and came back, a pointer entering and lifting the rate: each of
/// those is one deliberate close followed by one fresh dial.
///
/// Read as a single connection — which is what this was until 2026-09-21 —
/// that first EOF was the road's death: the bus closed, every later frame
/// answered `iOS 화면 스트림이 닫혔습니다`, three of those rested the fast road
/// for five seconds, and the pane fell to the still-picture road. Measured in
/// the window's own log that day: a pane pushed its first picture at +9.0s,
/// was resized, rested at +26.5s and only came back at +32.6s (1060x2304 ->
/// 412x896) — twenty-three seconds of a slow road bought by a resize.
fn serve_frame_socket(listener: &UnixListener, bus: &FrameBus) {
    // The first dial is the helper coming to work at all, and it has a
    // deadline: a helper that never connects must not leave waiters hoping.
    let Some(first) = accept_helper(listener) else {
        return;
    };
    read_frames_from(first, bus);
    // Every later one is a re-negotiation or a resume, and those have no
    // deadline — a paused pane can sit for minutes. Blocking rather than
    // polling for that wait: five milliseconds of looking, for minutes, is a
    // core spent on nothing. [`InputClient::drop`] knocks on this socket, so
    // the sleep ends when the client does.
    if listener.set_nonblocking(false).is_err() {
        return;
    }
    while !bus.is_closed() {
        let Ok((stream, _)) = listener.accept() else {
            return;
        };
        if bus.is_closed() {
            return;
        }
        read_frames_from(stream, bus);
    }
}

/// One connection's pictures, until the socket or the protocol gives out.
fn read_frames_from(stream: UnixStream, bus: &FrameBus) {
    // Back to blocking now that there IS a peer: the reader thread has nothing
    // to do but wait for bytes, and spinning on WouldBlock would burn a core
    // for the life of the pane.
    if stream.set_nonblocking(false).is_err() {
        return;
    }
    let mut socket = BufReader::new(stream);
    let mut buffer = Vec::new();
    let mut chunk = vec![0u8; FRAME_SOCKET_READ_CHUNK];
    loop {
        let read = match socket.read(&mut chunk) {
            Ok(0) | Err(_) => return,
            Ok(read) => read,
        };
        buffer.extend_from_slice(&chunk[..read]);
        match drain_frames(&mut buffer) {
            Ok(frames) => {
                for frame in frames {
                    bus.put(frame);
                }
            }
            Err(_) => return,
        }
    }
}

fn accept_helper(listener: &UnixListener) -> Option<UnixStream> {
    let deadline = Instant::now() + FRAME_SOCKET_ACCEPT_TIMEOUT;
    loop {
        match listener.accept() {
            Ok((stream, _)) => return Some(stream),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return None;
                }
                std::thread::sleep(FRAME_SOCKET_ACCEPT_POLL);
            }
            Err(_) => return None,
        }
    }
}

/// What the helper was last told to push, kept beside it so a replacement
/// helper is told the same before anyone is handed it: the pump says its size
/// once per size, not once per helper, and must not have to know that the
/// helper it told has since been replaced.
#[derive(Clone, Copy)]
struct StreamAsk {
    long_edge: u32,
    quality: f64,
    max_fps: u32,
}

struct ClientEntry {
    client: Arc<InputClient>,
    references: usize,
    stream: Option<StreamAsk>,
}

impl ClientEntry {
    /// A fresh helper in the corpse's place. The references the panes hold
    /// keep meaning what they meant, and the pictures the corpse was pushing
    /// are asked of the newcomer before it is handed out.
    fn replace_helper(&mut self, udid: &str) -> Result<Arc<InputClient>, String> {
        let fresh = InputClient::start(udid)?;
        if let Some(stream) = self.stream {
            fresh.request(&InputRequest::Stream {
                long_edge: stream.long_edge,
                quality: stream.quality,
                max_fps: stream.max_fps,
            })?;
        }
        self.client = fresh.clone();
        Ok(fresh)
    }
}

fn clients() -> &'static Mutex<HashMap<String, ClientEntry>> {
    static CLIENTS: OnceLock<Mutex<HashMap<String, ClientEntry>>> = OnceLock::new();
    CLIENTS.get_or_init(|| Mutex::new(HashMap::new()))
}

pub(super) fn retain(udid: &str) -> Result<(), String> {
    let mut clients = held(clients());
    if let Some(entry) = clients.get_mut(udid) {
        if entry.client.alive() {
            entry.references = entry.references.saturating_add(1);
            return Ok(());
        }
        // The helper behind this entry is gone. Panes still holding it keep
        // their references — releasing is still their job — but they are
        // holding a corpse either way, so the entry gets a live helper and
        // this pane's reference is added to the count that was already there.
        entry.replace_helper(udid)?;
        entry.references = entry.references.saturating_add(1);
        return Ok(());
    }
    let client = InputClient::start(udid)?;
    clients.insert(
        udid.to_string(),
        ClientEntry {
            client,
            references: 1,
            stream: None,
        },
    );
    Ok(())
}

/// Whether this device's helper has been asked for and is standing.
///
/// The pump is started before the capability thread retains its helper,
/// deliberately, so the first picture does not wait on a 247-495ms cold
/// start; that leaves a window where asking for a frame finds an empty map.
/// "Nobody has asked for one yet" and "the one we have is broken" are
/// different facts and the frame road spends its patience on only one of
/// them — which is why a CORPSE answers false here too: an entry whose
/// process has died would otherwise send the road through its whole miss
/// budget on a pipe that cannot answer, once per rest. The corpse itself is
/// replaced by the next successful `retain`, and the references its panes
/// still hold keep meaning what they meant.
pub(super) fn retained(udid: &str) -> bool {
    held(clients())
        .get(udid)
        .is_some_and(|entry| entry.client.alive())
}

pub(super) fn release(udid: &str) {
    let removed = {
        let mut clients = held(clients());
        let Some(entry) = clients.get_mut(udid) else {
            return;
        };
        entry.references = entry.references.saturating_sub(1);
        (entry.references == 0)
            .then(|| clients.remove(udid))
            .flatten()
    };
    drop(removed);
}

const NO_CONNECTION: &str = "이 iOS 시뮬레이터의 입력 연결이 없습니다";

/// This device's live helper, or the one sentence every road that needs one
/// says when there is none.
///
/// Four call sites were spelling the same lookup, the same `clone`, and the
/// same message; the fifth would have been the one that spelled it differently.
///
/// A corpse is not handed out. The entry a pane retained outlives the helper
/// it was retained with (a device that reboots takes the helper with it), and
/// until 2026-09-21 every verb after that death was handed the same dead pipe
/// and answered `Broken pipe` for as long as the pane stayed open. A helper
/// known to be dead is replaced here, once, before the caller is given one.
fn client_for(udid: &str) -> Result<Arc<InputClient>, String> {
    let mut clients = held(clients());
    let entry = clients
        .get_mut(udid)
        .ok_or_else(|| NO_CONNECTION.to_string())?;
    if entry.client.alive() {
        return Ok(entry.client.clone());
    }
    entry.replace_helper(udid)
}

/// The helper `failed` stood for, replaced — unless another road already
/// replaced it, in which case that newcomer is the answer.
fn resummoned(udid: &str, failed: &Arc<InputClient>) -> Result<Arc<InputClient>, String> {
    let mut clients = held(clients());
    let entry = clients
        .get_mut(udid)
        .ok_or_else(|| NO_CONNECTION.to_string())?;
    if !Arc::ptr_eq(&entry.client, failed) && entry.client.alive() {
        return Ok(entry.client.clone());
    }
    entry.replace_helper(udid)
}

/// One request to this device's helper, answered — asked a second time, of a
/// fresh helper, only when the first never left this process (the pipe was
/// already broken: the helper had died, unnoticed, before the ask). A request
/// the helper had — one that timed out, or came back malformed — is not asked
/// again: a tap it may have delivered must not be delivered twice.
fn ask_device(udid: &str, request: &InputRequest) -> Result<WireResponse, String> {
    let client = client_for(udid)?;
    match client.ask(request) {
        Ok(answer) => Ok(answer),
        Err(AskFailure::Unsent(why)) => {
            let fresh = resummoned(udid, &client).map_err(|error| format!("{why}; {error}"))?;
            fresh.ask(request).map_err(AskFailure::into_message)
        }
        Err(failure) => Err(failure.into_message()),
    }
}

/// One input to the device, and the pump told about it: the seconds after a
/// touch are the ones the pane is pushed at full rate for.
pub(super) fn send(udid: &str, request: InputRequest) -> Result<(), String> {
    send_unnoted(udid, request)?;
    super::session::registry().nudge(super::EmulatorPlatform::Ios, udid);
    Ok(())
}

fn send_unnoted(udid: &str, request: InputRequest) -> Result<(), String> {
    // The connection is asked about before the pasteboard is touched.
    client_for(udid)?;
    match request {
        InputRequest::Text { ref text } if !text.is_ascii() => {
            copy_to_simulator_pasteboard(udid, text)?;
            ask_device(udid, &InputRequest::Paste).map(|_| ())
        }
        request => ask_device(udid, &request).map(|_| ()),
    }
}

/// Ask the helper to start pushing pictures, at this size and quality.
///
/// Said once when a pane opens and again whenever its size or its pause state
/// changes — never per frame. That is the whole point: after this message the
/// request lane below is free for touches, and the pictures come up their own
/// socket without asking anyone's permission.
pub(super) fn stream_frames(
    udid: &str,
    long_edge: u32,
    quality: f64,
    max_fps: u32,
) -> Result<(), String> {
    ask_device(
        udid,
        &InputRequest::Stream {
            long_edge,
            quality,
            max_fps,
        },
    )?;
    if let Some(entry) = held(clients()).get_mut(udid) {
        entry.stream = Some(StreamAsk {
            long_edge,
            quality,
            max_fps,
        });
    }
    Ok(())
}

/// Ask the helper to stop pushing, so a hidden pane costs no encoder.
pub(super) fn stop_frames(udid: &str) -> Result<(), String> {
    ask_device(udid, &InputRequest::StreamStop)?;
    if let Some(entry) = held(clients()).get_mut(udid) {
        entry.stream = None;
    }
    Ok(())
}

/// The newest pushed picture, waiting up to `timeout` for one.
///
/// `Ok(None)` is a still screen and costs nothing; `Err` means the push socket
/// is gone and the caller should fall back to the roads that poll.
pub(super) fn next_frame(udid: &str, timeout: Duration) -> Result<Option<HelperFrame>, String> {
    client_for(udid)?.frames.take(timeout)
}

/// One picture asked for and waited on, or nothing when the screen has not
/// moved since `seed`.
///
/// The pulled road, kept for the two jobs the pushed one cannot do: proving a
/// quiet stream is still alive, and drawing at all on a machine whose frame
/// socket never came up. Measured against the roads BELOW it, per frame on this
/// machine: a CoreSimulator still picture costs 130-231ms and a `sips` downscale
/// another 52-79ms on top, while this answers in 1.92ms end to end — encode,
/// base64, JSON and pipe included — because the helper already holds the
/// device's framebuffer and its encoder.
pub(super) fn still_frame(
    udid: &str,
    seed: Option<u32>,
    long_edge: u32,
    quality: f64,
) -> Result<Option<HelperFrame>, String> {
    let answer = ask_device(
        udid,
        &InputRequest::Frame {
            seed,
            long_edge,
            quality,
        },
    )?;
    let Some(encoded) = answer.data else {
        // Unchanged. The helper still says which generation it looked at, so a
        // caller that lost its own seed is not stuck asking about a picture it
        // will never be told about.
        return Ok(None);
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|error| format!("iOS 화면을 읽지 못했습니다: {error}"))?;
    let seed = answer
        .seed
        .ok_or("iOS 화면에 세대 번호가 없습니다".to_string())?;
    Ok(Some(HelperFrame {
        bytes,
        seed,
        width: answer.width.unwrap_or_default(),
        height: answer.height.unwrap_or_default(),
    }))
}

pub(super) fn accessibility_tree(udid: &str) -> Result<serde_json::Value, String> {
    normalize_accessibility_tree(&accessibility_roots(udid)?)
}

/// Marks need the raw AX frame, before the display tree rounds it to 0..1.
pub(super) fn accessibility_roots(udid: &str) -> Result<Vec<serde_json::Value>, String> {
    let json = ask_device(udid, &InputRequest::Ax)?
        .data
        .ok_or("iOS 접근성 트리가 비어 있습니다")?;
    serde_json::from_str(&json)
        .map_err(|error| format!("iOS 접근성 트리를 읽지 못했습니다: {error}"))
}

#[derive(Clone, Copy)]
struct AxFrame {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn normalize_accessibility_tree(roots: &[serde_json::Value]) -> Result<serde_json::Value, String> {
    let screen = roots
        .first()
        .map(read_ax_frame)
        .filter(|frame| frame.width > 0.0 && frame.height > 0.0)
        .unwrap_or(AxFrame {
            x: 0.0,
            y: 0.0,
            width: 1.0,
            height: 1.0,
        });
    let mut remaining = 500usize;
    let mut normalized = Vec::new();
    for root in roots {
        if remaining == 0 {
            break;
        }
        normalized.push(normalize_ax_node(root, screen, &mut remaining));
    }
    serde_json::to_value(normalized).map_err(|error| error.to_string())
}

fn normalize_ax_node(
    raw: &serde_json::Value,
    screen: AxFrame,
    remaining: &mut usize,
) -> serde_json::Value {
    *remaining = remaining.saturating_sub(1);
    let object = raw.as_object();
    let raw_children = object
        .and_then(|node| node.get("children"))
        .and_then(serde_json::Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default();
    let mut children = Vec::new();
    for child in raw_children {
        if *remaining == 0 {
            break;
        }
        children.push(normalize_ax_node(child, screen, remaining));
    }
    let string = |key: &str| {
        object
            .and_then(|node| node.get(key))
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string()
    };
    let frame = read_ax_frame(raw);
    let mut normalized = serde_json::json!({
        "role": string("role_description"),
        "type": string("type"),
        "label": string("AXLabel"),
        "value": string("AXValue"),
        "enabled": object
            .and_then(|node| node.get("enabled"))
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(true),
        "frame": {
            "x": round4((frame.x - screen.x) / screen.width),
            "y": round4((frame.y - screen.y) / screen.height),
            "width": round4(frame.width / screen.width),
            "height": round4(frame.height / screen.height),
        },
        "children": children,
    });
    if let Some(id) = object
        .and_then(|node| node.get("AXUniqueId"))
        .and_then(serde_json::Value::as_str)
        .filter(|id| !id.is_empty())
    {
        normalized["id"] = id.into();
    }
    if children_len(&normalized) < raw_children.len() {
        normalized["truncated"] = true.into();
    }
    normalized
}

fn children_len(node: &serde_json::Value) -> usize {
    node.get("children")
        .and_then(serde_json::Value::as_array)
        .map(Vec::len)
        .unwrap_or_default()
}

fn read_ax_frame(node: &serde_json::Value) -> AxFrame {
    let frame = node.get("frame").and_then(serde_json::Value::as_object);
    let number = |key: &str| {
        frame
            .and_then(|frame| frame.get(key))
            .and_then(serde_json::Value::as_f64)
            .filter(|value| value.is_finite())
            .unwrap_or_default()
    };
    AxFrame {
        x: number("x"),
        y: number("y"),
        width: number("width"),
        height: number("height"),
    }
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

pub(super) fn shutdown_all() {
    let entries = held(clients())
        .drain()
        .map(|(_, entry)| entry)
        .collect::<Vec<_>>();
    drop(entries);
}

fn copy_to_simulator_pasteboard(udid: &str, text: &str) -> Result<(), String> {
    let mut child = super::ios::simctl_command()
        .args(["pbcopy", udid])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("시뮬레이터 클립보드를 열 수 없습니다: {error}"))?;
    child
        .stdin
        .take()
        .ok_or("시뮬레이터 클립보드 입력 문이 없습니다")?
        .write_all(text.as_bytes())
        .map_err(|error| format!("시뮬레이터 클립보드에 쓸 수 없습니다: {error}"))?;
    let deadline = Instant::now() + PASTEBOARD_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(()),
            Ok(Some(_)) => return Err("시뮬레이터 클립보드가 입력을 거절했습니다".to_string()),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(25));
            }
            _ => {
                stop_process(&mut child);
                return Err("시뮬레이터 클립보드 입력이 시간을 초과했습니다".to_string());
            }
        }
    }
}

/// The helper every client is started from: the embedded one — or, in this
/// crate's own tests, a stand-in that answers like it.
fn helper_program() -> Result<PathBuf, String> {
    #[cfg(test)]
    if let Some(program) = held(&HELPER_STAND_IN).clone() {
        return Ok(program);
    }
    helper_path().map(Path::to_path_buf)
}

#[cfg(test)]
static HELPER_STAND_IN: Mutex<Option<PathBuf>> = Mutex::new(None);

fn helper_path() -> Result<&'static Path, String> {
    static HELPER: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    HELPER
        .get_or_init(materialize_helper)
        .as_deref()
        .map_err(Clone::clone)
}

fn materialize_helper() -> Result<PathBuf, String> {
    use std::os::unix::fs::{DirBuilderExt as _, OpenOptionsExt as _, PermissionsExt as _};

    let mut digest = Sha256::new();
    digest.update(HELPER_BYTES);
    let hash = format!("{:x}", digest.finalize());
    let directory = std::env::temp_dir().join("zerocode-emulator-helper");
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true).mode(0o700);
    builder
        .create(&directory)
        .map_err(|error| format!("iOS 입력 헬퍼 디렉터리를 만들 수 없습니다: {error}"))?;
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| error.to_string())?;
    let destination = directory.join(format!("ios-input-{}", &hash[..16]));
    if std::fs::read(&destination).is_ok_and(|bytes| bytes == HELPER_BYTES) {
        std::fs::set_permissions(&destination, std::fs::Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())?;
        return Ok(destination);
    }

    let staging = directory.join(format!(".ios-input-{}", uuid::Uuid::new_v4()));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true).mode(0o600);
    match options.open(&staging) {
        Ok(mut file) => {
            file.write_all(HELPER_BYTES)
                .map_err(|error| error.to_string())?;
            file.sync_all().map_err(|error| error.to_string())?;
            std::fs::set_permissions(&staging, std::fs::Permissions::from_mode(0o700))
                .map_err(|error| error.to_string())?;
            std::fs::rename(&staging, &destination).map_err(|error| error.to_string())?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            // Another window is materializing the same content-addressed file.
            let deadline = Instant::now() + Duration::from_secs(2);
            while !destination.is_file() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
            if !destination.is_file() {
                return Err("iOS 입력 헬퍼를 준비하지 못했습니다".to_string());
            }
        }
        Err(error) => return Err(error.to_string()),
    }
    Ok(destination)
}

fn stop_process(child: &mut Child) {
    let _ = child.kill();
    let _ = child.wait();
}

fn held<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt as _;

    #[test]
    fn helper_is_content_addressed_and_executable() {
        let first = helper_path().expect("materialized helper").to_path_buf();
        let second = helper_path().expect("cached helper").to_path_buf();
        assert_eq!(first, second);
        assert_eq!(
            first.metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );
    }

    #[test]
    fn wire_request_has_one_id_and_a_tagged_payload() {
        let wire = serde_json::to_value(WireRequest {
            id: 7,
            request: &InputRequest::Swipe {
                x1: 0.1,
                y1: 0.2,
                x2: 0.3,
                y2: 0.4,
                duration_ms: 250,
            },
        })
        .unwrap();
        assert_eq!(wire["id"], 7);
        assert_eq!(wire["kind"], "swipe");
        assert_eq!(wire["durationMs"], 250);

        let multi = serde_json::to_value(WireRequest {
            id: 8,
            request: &InputRequest::MultiTouch {
                x1: 0.4,
                y1: 0.5,
                x2: 0.6,
                y2: 0.5,
                x3: 0.3,
                y3: 0.5,
                x4: 0.7,
                y4: 0.5,
                duration_ms: 160,
            },
        })
        .unwrap();
        assert_eq!(multi["kind"], "multitouch");
        assert_eq!(multi["x4"], 0.7);
    }

    fn pushed(seed: u32, width: u32, height: u32, body: &[u8]) -> Vec<u8> {
        let mut wire = Vec::new();
        wire.extend_from_slice(&(body.len() as u32).to_be_bytes());
        wire.extend_from_slice(&seed.to_be_bytes());
        wire.extend_from_slice(&width.to_be_bytes());
        wire.extend_from_slice(&height.to_be_bytes());
        wire.extend_from_slice(body);
        wire
    }

    /// A pusher that retires and dials again is a seam in the road, not its
    /// end.
    ///
    /// The helper opens one connection per STREAM: re-negotiating a size
    /// retires the old pusher thread, which closes its end, and starts a new
    /// one that dials afresh. Served as a single connection, that first EOF
    /// closed the bus for good and every later frame answered "the stream is
    /// closed" — three of which rest the fast road for five seconds.
    #[test]
    fn a_pusher_that_retires_and_dials_again_keeps_the_same_bus() {
        let socket = std::env::temp_dir().join(format!(
            "zc-ios-test-{}.sock",
            &uuid::Uuid::new_v4().simple().to_string()[..12]
        ));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket).expect("a socket to listen on");
        listener
            .set_nonblocking(true)
            .expect("a listener that can be polled");
        let bus = Arc::new(FrameBus::new());
        let serving = {
            let bus = bus.clone();
            std::thread::spawn(move || {
                serve_frame_socket(&listener, &bus);
                bus.close();
            })
        };
        // Two streams in a row, each closed by the pusher the way a
        // re-negotiation closes one.
        for seed in [7u32, 8] {
            let mut dialled = UnixStream::connect(&socket).expect("the helper dials");
            dialled
                .write_all(&pushed(seed, 414, 900, b"picture"))
                .expect("one picture");
            drop(dialled);
            let frame = bus
                .take(Duration::from_secs(5))
                .expect("the bus is still open")
                .expect("the picture arrives");
            assert_eq!(frame.seed, seed, "the second stream never reached the bus");
        }
        // And the way the client itself ends it: the bus closed, then one
        // knock so a thread asleep in `accept` goes home.
        bus.close();
        let _ = UnixStream::connect(&socket);
        serving
            .join()
            .expect("the frame thread ends with the client");
        let _ = std::fs::remove_file(&socket);
    }

    #[test]
    fn one_read_carrying_several_whole_pictures_yields_all_of_them() {
        let mut buffer = pushed(7, 414, 900, b"first");
        buffer.extend(pushed(8, 414, 900, b"second"));
        buffer.extend(pushed(9, 414, 900, b"third"));
        let frames = drain_frames(&mut buffer).expect("three whole pictures");
        assert_eq!(frames.len(), 3);
        assert_eq!(frames[0].bytes, b"first");
        assert_eq!(frames[0].seed, 7);
        assert_eq!(frames[2].bytes, b"third");
        assert_eq!(frames[2].seed, 9);
        // Everything was consumed, so the next read starts clean.
        assert!(buffer.is_empty());
    }

    #[test]
    fn a_header_split_across_two_reads_waits_instead_of_being_guessed_at() {
        let whole = pushed(3, 414, 900, b"picture");
        let mut buffer = whole[..9].to_vec();
        assert!(
            drain_frames(&mut buffer)
                .expect("a short read is not an error")
                .is_empty(),
            "half a header is not a picture"
        );
        // The tail is kept verbatim — dropping it would lose the picture.
        assert_eq!(buffer, whole[..9]);
        buffer.extend_from_slice(&whole[9..]);
        let frames = drain_frames(&mut buffer).expect("the completed picture");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].bytes, b"picture");
        assert!(buffer.is_empty());
    }

    #[test]
    fn a_body_that_has_not_all_arrived_yet_leaves_its_whole_frame_pending() {
        let whole = pushed(4, 414, 900, b"a longer picture body");
        let split = whole.len() - 5;
        let mut buffer = whole[..split].to_vec();
        assert!(drain_frames(&mut buffer).expect("short body").is_empty());
        assert_eq!(buffer.len(), split);
        buffer.extend_from_slice(&whole[split..]);
        let frames = drain_frames(&mut buffer).expect("the completed picture");
        assert_eq!(frames[0].bytes, b"a longer picture body");
    }

    #[test]
    fn a_length_that_could_never_be_a_picture_ends_the_connection() {
        // Waiting cannot make this sane, so it must be told apart from a short
        // read — otherwise the reader spins on the same bytes forever.
        let mut absurd = Vec::new();
        absurd.extend_from_slice(&(MAX_PUSH_FRAME_BYTES as u32 + 1).to_be_bytes());
        absurd.extend_from_slice(&1u32.to_be_bytes());
        absurd.extend_from_slice(&414u32.to_be_bytes());
        absurd.extend_from_slice(&900u32.to_be_bytes());
        assert!(drain_frames(&mut absurd).is_err());

        let mut empty = pushed(1, 414, 900, b"");
        assert!(drain_frames(&mut empty).is_err());

        let mut sizeless = pushed(1, 0, 900, b"body");
        assert!(drain_frames(&mut sizeless).is_err());
    }

    #[test]
    fn the_bus_keeps_only_the_newest_picture_so_a_slow_pane_skips_to_now() {
        let bus = FrameBus::new();
        for seed in 1..=3u32 {
            bus.put(HelperFrame {
                bytes: vec![seed as u8],
                seed,
                width: 414,
                height: 900,
            });
        }
        let frame = bus
            .take(Duration::from_millis(0))
            .expect("a live bus")
            .expect("the newest picture");
        assert_eq!(
            frame.seed, 3,
            "a mirror is only ever worth its latest frame"
        );
        // Drained: a still screen answers nothing rather than the same picture.
        assert!(
            bus.take(Duration::from_millis(0))
                .expect("still live")
                .is_none()
        );
    }

    #[test]
    fn a_closed_bus_says_so_instead_of_making_a_pane_wait_forever() {
        let bus = FrameBus::new();
        bus.close();
        assert!(bus.take(Duration::from_millis(0)).is_err());

        // A picture already on the bus is still handed over after the close —
        // the last thing the helper managed to push is not thrown away.
        let live = FrameBus::new();
        live.put(HelperFrame {
            bytes: vec![1],
            seed: 5,
            width: 414,
            height: 900,
        });
        live.close();
        assert_eq!(
            live.take(Duration::from_millis(0))
                .expect("the pending picture")
                .expect("a picture")
                .seed,
            5
        );
        assert!(live.take(Duration::from_millis(0)).is_err());
    }

    #[test]
    fn ios_accessibility_frames_are_normalized_for_direct_input_reuse() {
        let raw = serde_json::json!([{
            "AXLabel": "Settings",
            "role_description": "application",
            "type": "Application",
            "enabled": true,
            "frame": { "x": 10.0, "y": 20.0, "width": 400.0, "height": 800.0 },
            "children": [{
                "AXUniqueId": "button",
                "AXLabel": "General",
                "frame": { "x": 110.0, "y": 220.0, "width": 200.0, "height": 80.0 },
                "children": []
            }]
        }]);
        let normalized = normalize_accessibility_tree(raw.as_array().unwrap()).unwrap();
        assert_eq!(normalized[0]["frame"]["width"], 1.0);
        assert_eq!(normalized[0]["children"][0]["frame"]["x"], 0.25);
        assert_eq!(normalized[0]["children"][0]["frame"]["y"], 0.25);
        assert_eq!(normalized[0]["children"][0]["id"], "button");
    }

    /// A helper that answers like the real one and dies as told by its udid,
    /// `stand-in-<mode>-<n>`: `leave` answers `n` requests and exits — dead
    /// before anyone asks again; `swallow` answers `n`, then reads the next
    /// request and exits without answering — a request the helper had.
    /// Every life logs what it was asked, so a test can read what a
    /// replacement was told before anyone asked it anything.
    const STAND_IN_SCRIPT: &str = r#"#!/bin/sh
dir='__DIR__'
udid="$1"
die="${udid##*-}"
rest="${udid%-*}"
mode="${rest##*-}"
n=$(cat "$dir/$udid.count" 2>/dev/null || echo 0)
n=$((n+1))
echo "$n" > "$dir/$udid.count"
count=0
while IFS= read -r line; do
  printf 'life %s: %s\n' "$n" "$line" >> "$dir/$udid.log"
  count=$((count+1))
  [ "$count" -gt "$die" ] && exit 0
  id=$(printf '%s' "$line" | sed -e 's/.*"id":\([0-9][0-9]*\).*/\1/')
  printf '{"id":%s,"ok":true,"data":"life-%s"}\n' "$id" "$n"
  if [ "$mode" = leave ] && [ "$count" -ge "$die" ]; then exit 0; fi
done
"#;

    fn stand_in() -> &'static Path {
        static STAND_IN: OnceLock<(tempfile::TempDir, PathBuf)> = OnceLock::new();
        let (dir, program) = STAND_IN.get_or_init(|| {
            let dir = tempfile::tempdir().expect("scratch");
            let program = dir.path().join("stand-in-helper");
            std::fs::write(
                &program,
                STAND_IN_SCRIPT.replace("__DIR__", &dir.path().to_string_lossy()),
            )
            .expect("stand-in script");
            std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700))
                .expect("chmod");
            *held(&HELPER_STAND_IN) = Some(program.clone());
            (dir, program)
        });
        let _ = dir;
        program
    }

    fn stand_in_log(udid: &str) -> String {
        let dir = stand_in().parent().expect("scratch").to_path_buf();
        std::fs::read_to_string(dir.join(format!("{udid}.log"))).unwrap_or_default()
    }

    fn wait_for_exit(client: &InputClient) {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if held(&client.state)
                .child
                .try_wait()
                .ok()
                .flatten()
                .is_some()
            {
                return;
            }
            assert!(Instant::now() < deadline, "the stand-in never left");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn a_helper_that_died_unnoticed_is_replaced_once_and_told_its_stream_again() {
        stand_in();
        // Each life answers three times — the start's ping, then two more —
        // and leaves.
        let udid = "stand-in-leave-3";
        retain(udid).expect("first helper");
        stream_frames(udid, 320, 0.5, 30).expect("stream asked");
        let first = client_for(udid).expect("the first helper");
        ask_device(udid, &InputRequest::Ping).expect("its third answer");
        wait_for_exit(&first);
        // Nobody has noticed: the entry still says its helper stands.
        assert!(retained(udid) && first.alive());
        let answer = ask_device(udid, &InputRequest::Ping).expect("asked again of a fresh helper");
        assert_eq!(answer.data.as_deref(), Some("life-2"));
        let second = client_for(udid).expect("the replacement");
        assert!(!Arc::ptr_eq(&first, &second));
        assert!(!first.alive() && second.alive());
        let log = stand_in_log(udid);
        let second_life: Vec<&str> = log
            .lines()
            .filter(|line| line.starts_with("life 2: "))
            .collect();
        assert_eq!(second_life.len(), 3, "{log}");
        assert!(second_life[0].contains(r#""kind":"ping""#), "{log}");
        assert!(
            second_life[1].contains(r#""kind":"stream""#)
                && second_life[1].contains(r#""longEdge":320"#)
                && second_life[1].contains(r#""maxFps":30"#),
            "the replacement was not told the stream first: {log}"
        );
        assert!(second_life[2].contains(r#""kind":"ping""#), "{log}");
        assert_eq!(held(clients())[udid].references, 1);
        release(udid);
        assert!(client_for(udid).is_err());
    }

    #[test]
    fn a_request_the_helper_had_is_not_asked_again() {
        stand_in();
        // Two answers a life — the start's ping and one more; the third ask
        // is read and never answered: the helper had it, so nothing is
        // retried.
        let udid = "stand-in-swallow-2";
        retain(udid).expect("first helper");
        let first = client_for(udid).expect("the first helper");
        ask_device(udid, &InputRequest::Ping).expect("its second answer");
        let error = ask_device(udid, &InputRequest::Ping)
            .err()
            .expect("no answer comes");
        assert!(error.contains("응답하지 않습니다"), "{error}");
        assert!(!first.alive());
        assert!(!stand_in_log(udid).contains("life 2"), "a retry was made");
        // The next ask finds the corpse and replaces it before asking.
        let answer = ask_device(udid, &InputRequest::Ping).expect("a fresh helper");
        assert_eq!(answer.data.as_deref(), Some("life-2"));
        release(udid);
    }

    #[test]
    fn a_corpse_is_replaced_before_it_is_handed_out_and_nobody_gets_a_missing_device() {
        stand_in();
        let udid = "stand-in-leave-9";
        retain(udid).expect("first helper");
        let first = client_for(udid).expect("the first helper");
        first.fell_over(&mut held(&first.state));
        assert!(!retained(udid));
        let second = client_for(udid).expect("a replacement, not the corpse");
        assert!(!Arc::ptr_eq(&first, &second) && second.alive() && retained(udid));
        assert_eq!(held(clients())[udid].references, 1);
        release(udid);
        assert_eq!(
            client_for("stand-in-nobody-1").err().as_deref(),
            Some(NO_CONNECTION)
        );
    }
}
