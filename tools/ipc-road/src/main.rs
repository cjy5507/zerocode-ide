//! The window's custom-scheme road, measured on its own (t-20972).
//!
//! Every `invoke` of the window is a `fetch("ipc://localhost/<command>")`, and
//! WebKit hands each one to the UI process's MAIN thread (`wry … url_scheme_handler
//! ::start_task`): it has WebKit rebuild the request as an NSURLRequest, copies
//! the body into a `Vec`, rebuilds the headers into an `http::HeaderMap`, and
//! calls the app's handler — which for Tauri parses a JSON body into a
//! `serde_json::Value` before it hands the command to its own thread. This
//! tool stands up that road with wry and tao at the versions the window ships,
//! sends requests shaped like Tauri's (same headers, same body encodings, a
//! page on one custom scheme calling another, so WebKit preflights as it does
//! for the window) and reads what the main thread paid for each from wry's own
//! `tracing` spans, with nothing patched:
//!
//!   handle        — all of `start_task`: conversion plus the app's handler;
//!   call_handler  — only the app's handler (here: Tauri's JSON parse, then a
//!                   hand-off of the reply to another thread).
//!
//! `handle − call_handler` is what WebKit and wry spend before the app sees the
//! request. It also records how long the main thread's run loop stood still
//! (the gap between two of its turns, which is what the window's hang watchdog
//! calls a hang) and what an `evaluate_script` of N bytes costs it — the
//! other road a payload takes into the page, the one an event rides.
//!
//!   ipc-road --out <result.json> [--quick] [--limit-secs <n>]
//!
//! A second mode measures the other thing a window's main thread pays for on
//! its own account: a webview being BORN beside the main one and loading a page
//! (the artifact gallery's hidden thumbnail pane is one):
//!
//!   ipc-road --out <result.json> --birth <calm|busy|heavy|layers>:<parked|hidden|none>
//!
//! It makes a child webview in the window's own view tree, parked far outside
//! the window the way the thumbnail pane is (or hidden, or not made at all),
//! loads a page into it (`busy` blocks its own page's script for seven
//! seconds; `heavy` is a megabyte of markup and a canvas animation; `layers` is three
//! thousand compositing layers moved by script on every frame), and says
//! how long the main thread's run loop stood still before, during and after.
//!
//! A third mode runs the artifact gallery's thumbnail pane the way the window
//! runs it — a child webview parked outside the window, on the same data store
//! the window names, a page loaded by `file://`, one snapshot taken by the
//! window's own `artifact_webkit` code, the pane sent back to a blank page — three
//! rounds, and says how long the main thread's run loop stood still in each step:
//!
//!   ipc-road --out <result.json> --thumb <calm|busy|heavy|layers|storm[N]|stormfix[N]>[:store][:window][:poke] [--assert-stall]
//!
//! With `--assert-stall` the run exits 4 when the run loop stood still longer
//! than `thumb::STALL_BOUND_MS` in any step of any round, and says which.
//!
//! Three pages of that mode are the window's own, as a webview the backend does
//! not know sees it: `storm` (with the fault reporter the window had) and
//! `stormfix` (with the one it has) are small pages that leave refused calls
//! uncaught; `ui` is the window's real `ui/index.html` (`--ui-page`) behind a
//! Tauri-like shim, and `uicut` the same page with the script the window gives
//! its hidden panes (`ui/hidden-pane-no-bridge.js`) run after the shim. In all of
//! them the pane's `ipc` road refuses every command on the main thread, as Tauri
//! does.
//!
//! It opens no window anywhere a person could see (the one it needs sits
//! outside every screen), reaches no network and reads nothing but the pages
//! it writes itself; the person's windows are not on any road it takes.

#[cfg(not(target_os = "macos"))]
compile_error!("ipc-road measures WKWebView's scheme road and builds on macOS only");

/// The window's own snapshot code, unchanged: raw objc2 on one WKWebView, with
/// nothing of the product around it (its header says it is made to be run by a
/// harness like this one).
#[allow(dead_code)]
#[path = "../../../crates/zerocode-shell/src/artifact_webkit.rs"]
mod artifact_webkit;
mod thumb;

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU64, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tao::event::Event;
use tao::event_loop::{ControlFlow, EventLoop};
use tao::platform::macos::{ActivationPolicy, EventLoopExtMacOS};
use tao::window::WindowBuilder;
use tracing::span::{Attributes, Id, Record};
use tracing::{Event as TraceEvent, Metadata, Subscriber};
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::http::{Method, Request, Response, header::CONTENT_TYPE};
use wry::{PageLoadEvent, Rect, RequestAsyncResponder, WebView, WebViewBuilder};

/// wry's span around the whole of `start_task`.
const HANDLE: &str = "wry::custom_protocol::handle";
/// wry's span around the call into the app's handler alone.
const CALL: &str = "wry::custom_protocol::call_handler";
/// The paths the page uses to talk to the tool itself rather than to measure.
const CONTROL: &str = "/__";
/// The page (`src/page.html`), served on the window's own scheme.
const PAGE: &str = include_str!("page.html");
/// How long the whole run may take before it gives up and says what it has.
const DEFAULT_LIMIT_SECS: u64 = 900;
/// How often the main thread asks to be woken when nothing happens: the floor
/// under every gap this tool reads.
const WAKE_EVERY: Duration = Duration::from_millis(1);
/// Where the birth mode's parent window stands: outside every screen, but a
/// window all the same, so AppKit treats its views as a window's.
const PARENT_AT: f64 = -6000.0;
/// Where a parked child stands inside the parent, and how big it is: the
/// thumbnail pane's own numbers (`artifact_thumbs::PARKED_AT`).
const CHILD_PARKED_AT: f64 = -20_000.0;
const CHILD_SIZE: (f64, f64) = (1200.0, 900.0);
/// How long the birth mode watches the run loop before the child is made and
/// after (the busy page holds its script for seven seconds).
const BIRTH_BEFORE: Duration = Duration::from_millis(1500);
const BIRTH_AFTER: Duration = Duration::from_millis(12_000);
/// The main page of the birth mode: nothing to say.
const BIRTH_MAIN_PAGE: &str =
    "<!doctype html><meta charset=\"utf-8\"><title>main</title><p>main</p>";
/// The window's fault reporter as it was before t-20972 (kept verbatim), and the
/// window's script as it is now, from which the reporter it has today is read.
const FAULT_REPORTER_BEFORE: &str = include_str!("fault_reporter_before.js");
const SHELL_BOOT: &str = include_str!("../../../ui/shell-boot.js");
/// What the window's hidden pane is told before its page's first script, as the
/// window embeds it: the `uicut` page runs it after the Tauri-like shim.
const NO_BRIDGE: &str = include_str!("../../../ui/hidden-pane-no-bridge.js");
/// How long a storm page keeps asking, and how many refused calls its boot
/// leaves uncaught by default (each one is a request chain while the reporter
/// is the old one; `storm32` says another number). The window's own boot makes
/// 65 to 100 calls in its first second and a half (the IPC census): eight is a
/// floor, not an estimate.
const STORM_MS: u32 = 6_000;
const STORM_BOOT_CALLS: u32 = 8;
const BUSY_PAGE: &str = "<!doctype html><meta charset=\"utf-8\"><script>const t=Date.now();while(Date.now()-t<7000){}</script><p>done</p>";
const CALM_PAGE: &str = "<!doctype html><meta charset=\"utf-8\"><p>calm</p>";

/// One request the main thread served inside a phase.
struct Sample {
    total_ns: u64,
    call_ns: u64,
    body: usize,
    preflight: bool,
}

/// A stretch of the page's script that is measured as one thing.
struct Phase {
    name: String,
    requests: Vec<Sample>,
    gaps_ns: Vec<u64>,
}

/// What the handler said about the request `start_task` is serving, read when
/// its `handle` span closes.
#[derive(Default, Clone, Copy)]
struct Serving {
    body: usize,
    preflight: bool,
    control: bool,
    call_ns: u64,
}

thread_local! {
    /// The spans open on this thread, innermost last.
    static OPEN: RefCell<Vec<(u64, Instant)>> = const { RefCell::new(Vec::new()) };
    static SERVING: RefCell<Serving> = RefCell::new(Serving::default());
    static WEBVIEW: RefCell<Option<WebView>> = const { RefCell::new(None) };
    /// What the pane's refusing `ipc` road was asked for, by command: it runs
    /// on the main thread, so the count needs no lock.
    static REFUSED: RefCell<HashMap<String, u64>> = RefCell::new(HashMap::new());
}

static PHASE: Mutex<Option<Phase>> = Mutex::new(None);
static RESULTS: Mutex<Vec<Value>> = Mutex::new(Vec::new());
static DONE: AtomicBool = AtomicBool::new(false);
/// What the process exits with once the event loop stops (`finish`'s code).
static EXIT_CODE: AtomicI32 = AtomicI32::new(0);
static BIRTH_MODE: AtomicBool = AtomicBool::new(false);
/// When the child's page said it had finished loading, and which page it was.
static LOADED_AT: Mutex<Option<(Instant, String)>> = Mutex::new(None);
static SYNC_REPLY: AtomicBool = AtomicBool::new(false);
/// The page the `ui` thumbnail pane shows: the window's own `ui/index.html`,
/// named on the command line (`--ui-page`).
static UI_PAGE: OnceLock<String> = OnceLock::new();
/// Every request `start_task` served, counted whether or not a phase was open,
/// and the time the main thread spent in them: the thumbnail mode reads the
/// totals per round.
static SERVED: AtomicU64 = AtomicU64::new(0);
static SERVED_NS: AtomicU64 = AtomicU64::new(0);
static OUT: OnceLock<String> = OnceLock::new();
static REPLIES: OnceLock<Mutex<Sender<(RequestAsyncResponder, Response<Vec<u8>>)>>> =
    OnceLock::new();

/// Reads wry's two spans: nothing else is enabled, so nothing else is paid for.
struct Road;

impl Subscriber for Road {
    fn enabled(&self, metadata: &Metadata<'_>) -> bool {
        metadata.name() == HANDLE || metadata.name() == CALL
    }

    fn new_span(&self, attributes: &Attributes<'_>) -> Id {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        // The low bit says which span this is: a request has one of each.
        let call = u64::from(attributes.metadata().name() == CALL);
        Id::from_u64((NEXT.fetch_add(1, Ordering::Relaxed) << 1) | call)
    }

    fn record(&self, _span: &Id, _values: &Record<'_>) {}

    fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

    fn event(&self, _event: &TraceEvent<'_>) {}

    fn enter(&self, span: &Id) {
        OPEN.with(|open| open.borrow_mut().push((span.into_u64(), Instant::now())));
    }

    fn exit(&self, span: &Id) {
        let ended = Instant::now();
        let id = span.into_u64();
        let started = OPEN.with(|open| {
            let mut open = open.borrow_mut();
            let at = open.iter().rposition(|(held, _)| *held == id)?;
            Some(open.remove(at).1)
        });
        let Some(started) = started else { return };
        let ns = u64::try_from(ended.duration_since(started).as_nanos()).unwrap_or(u64::MAX);
        if id & 1 == 1 {
            SERVING.with(|serving| serving.borrow_mut().call_ns = ns);
        } else {
            request_served(ns);
        }
    }
}

/// `start_task` returned: put the request into the phase that is open, unless
/// it was the page talking to the tool.
fn request_served(total_ns: u64) {
    let served = SERVING.with(|serving| std::mem::take(&mut *serving.borrow_mut()));
    if served.control {
        return;
    }
    SERVED.fetch_add(1, Ordering::Relaxed);
    SERVED_NS.fetch_add(total_ns, Ordering::Relaxed);
    if let Some(phase) = PHASE.lock().unwrap().as_mut() {
        phase.requests.push(Sample {
            total_ns,
            call_ns: served.call_ns,
            body: served.body,
            preflight: served.preflight,
        });
    }
}

fn percentile(sorted: &[u64], share: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let at = (sorted.len() as f64 * share).floor() as usize;
    sorted[at.min(sorted.len() - 1)]
}

fn microseconds(ns: u64) -> f64 {
    (ns as f64 / 100.0).round() / 10.0
}

/// p50, p95 and max of nanosecond readings, in microseconds.
fn spread(mut readings: Vec<u64>) -> Value {
    readings.sort_unstable();
    json!({
        "n": readings.len(),
        "p50": microseconds(percentile(&readings, 0.5)),
        "p95": microseconds(percentile(&readings, 0.95)),
        "max": microseconds(readings.last().copied().unwrap_or(0)),
    })
}

fn open_phase(name: &str) {
    *PHASE.lock().unwrap() = Some(Phase {
        name: name.to_string(),
        requests: Vec::new(),
        gaps_ns: Vec::new(),
    });
}

fn close_phase(wall_ms: f64) -> Value {
    let Some(phase) = PHASE.lock().unwrap().take() else {
        return Value::Null;
    };
    let posts: Vec<&Sample> = phase.requests.iter().filter(|s| !s.preflight).collect();
    let preflights: Vec<&Sample> = phase.requests.iter().filter(|s| s.preflight).collect();
    let main_thread_ns: u64 = phase.requests.iter().map(|s| s.total_ns).sum();
    let mut gaps = phase.gaps_ns;
    gaps.sort_unstable();
    json!({
        "phase": phase.name,
        "requests": phase.requests.len(),
        "posts": posts.len(),
        "preflights": preflights.len(),
        "body_bytes": posts.iter().map(|s| s.body).max().unwrap_or(0),
        "post_total_us": spread(posts.iter().map(|s| s.total_ns).collect()),
        "post_conversion_us": spread(
            posts.iter().map(|s| s.total_ns.saturating_sub(s.call_ns)).collect(),
        ),
        "post_handler_us": spread(posts.iter().map(|s| s.call_ns).collect()),
        "preflight_total_us": spread(preflights.iter().map(|s| s.total_ns).collect()),
        "main_thread_ms": (main_thread_ns as f64 / 1e4).round() / 100.0,
        "wall_ms": (wall_ms * 10.0).round() / 10.0,
        "loop_gap_ms": {
            "p95": microseconds(percentile(&gaps, 0.95)) / 1000.0,
            "max": microseconds(gaps.last().copied().unwrap_or(0)) / 1000.0,
        },
    })
}

/// What the main thread pays to hand the page `bytes` bytes of script — the road
/// an event takes — read `reps` times.
fn eval_phase(bytes: usize, reps: usize) -> Value {
    let script = format!("window.__sink&&window.__sink(\"{}\")", "x".repeat(bytes));
    let mut readings = Vec::with_capacity(reps);
    WEBVIEW.with(|held| {
        let held = held.borrow();
        let Some(webview) = held.as_ref() else { return };
        for _ in 0..reps {
            let started = Instant::now();
            let _ = webview.evaluate_script(&script);
            readings.push(u64::try_from(started.elapsed().as_nanos()).unwrap_or(u64::MAX));
        }
    });
    json!({ "phase": format!("eval-{bytes}"), "eval_us": spread(readings), "body_bytes": bytes })
}

/// A megabyte of markup and style and a canvas that animates: a page whose
/// WebContent process has plenty to do.
fn heavy_page() -> String {
    let mut html = String::from("<!doctype html><meta charset=\"utf-8\"><style>");
    for n in 0..3000 {
        html.push_str(&format!(
            ".c{n}{{padding:{}px;margin:{}px;border:1px solid hsl({},50%,50%);background:linear-gradient(90deg,hsl({},60%,60%),hsl({},60%,40%))}}",
            n % 7,
            n % 5,
            n % 360,
            n % 360,
            (n + 90) % 360
        ));
    }
    html.push_str("</style><body>");
    for n in 0..9000 {
        html.push_str(&format!(
            "<div class=c{}>node {n} lorem ipsum dolor sit amet consectetur</div>",
            n % 3000
        ));
    }
    html.push_str(
        "<canvas id=k width=600 height=400></canvas><script>const k=document.getElementById('k').getContext('2d');let t=0;function f(){t+=1;k.clearRect(0,0,600,400);for(let i=0;i<800;i+=1){k.fillStyle='hsl('+((i*7+t)%360)+',70%,50%)';k.fillRect((i*13+t*3)%600,(i*29)%400,20,20)}requestAnimationFrame(f)}f();</script>",
    );
    html
}

/// Three thousand boxes, each on a compositing layer of its own (`will-change`),
/// moved by script on every frame: a dashboard of animated cards. WebKit's
/// WebContent process draws it, but the UI process applies every frame's layer
/// tree to the window's own Core Animation layers on the main thread, so a page
/// shaped like this is the one that could cost the main thread something
/// while it sits parked.
fn layers_page() -> String {
    let mut html = String::from(
        "<!doctype html><meta charset=\"utf-8\"><style>body{margin:0;overflow:hidden}.l{position:absolute;width:48px;height:48px;border-radius:8px;will-change:transform}</style><body>",
    );
    for n in 0..3000 {
        html.push_str(&format!(
            "<div class=l style=\"left:{}px;top:{}px;background:hsl({},60%,50%)\"></div>",
            (n * 37) % 1000,
            (n * 53) % 720,
            n % 360
        ));
    }
    html.push_str(
        "<script>const e=[...document.querySelectorAll('.l')];let t=0;function f(){t+=1;for(let i=0;i<e.length;i+=1){e[i].style.transform='translate('+Math.sin((t+i)/20)*30+'px,'+Math.cos((t+i)/25)*30+'px) rotate('+((t+i)%360)+'deg)'}requestAnimationFrame(f)}f();</script>",
    );
    html
}

/// What Tauri hands every webview, as far as the window's own scripts read it:
/// `window.__TAURI__.core.invoke` (a `fetch` of `ipc://localhost/<command>` with
/// Tauri's own headers), `Channel`, `event.listen` (an `invoke` of
/// `plugin:event|listen`, as the real one is), the clipboard plugin, and the
/// label of the webview. The `ui` pane runs it before the page's first script.
const TAURI_SHIM: &str = r#"(() => {
  const invoke = (command, args) => fetch('ipc://localhost/' + command, {
    method: 'POST',
    headers: { 'Content-Type': 'application/json', 'Tauri-Callback': '1', 'Tauri-Error': '2', 'Tauri-Invoke-Key': 'k' },
    body: JSON.stringify(args ?? {}),
  }).then(async (reply) => { const text = await reply.text(); if (!reply.ok) throw text; return text; });
  class Channel { constructor(onmessage) { this.onmessage = onmessage; this.id = 1; } toJSON() { return '__CHANNEL__:' + this.id; } }
  const listen = (event) => invoke('plugin:event|listen', { event, target: { kind: 'Any' }, handler: 1 }).then(() => () => {});
  window.__TAURI_INTERNALS__ = { metadata: { currentWebview: { label: 'artifact-thumb' }, currentWindow: { label: 'main' } } };
  window.__TAURI__ = {
    core: { invoke, Channel },
    event: { listen },
    clipboardManager: {
      readText: () => invoke('plugin:clipboard-manager|read_text'),
      writeText: (text) => invoke('plugin:clipboard-manager|write_text', { text }),
    },
  };
})();"#;

/// The commands a refusing road was asked for since `earlier`, most asked first.
fn refused_since(earlier: &HashMap<String, u64>, top: usize) -> Vec<(String, u64)> {
    let mut rows: Vec<(String, u64)> = REFUSED.with(|held| {
        held.borrow()
            .iter()
            .map(|(command, count)| {
                (
                    command.clone(),
                    count.saturating_sub(earlier.get(command).copied().unwrap_or(0)),
                )
            })
            .filter(|(_, count)| *count > 0)
            .collect()
    });
    rows.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    rows.truncate(top);
    rows
}

fn refused_snapshot() -> HashMap<String, u64> {
    REFUSED.with(|held| held.borrow().clone())
}

/// The window's fault reporter as it stands in `ui/shell-boot.js` today: from
/// `describeWindowFault` up to the next block's comment.
fn fault_reporter_now() -> &'static str {
    let from = SHELL_BOOT
        .find("function describeWindowFault")
        .expect("shell-boot.js holds the window's fault reporter");
    let to = SHELL_BOOT[from..]
        .find("/* One idle-aware poller")
        .map_or(SHELL_BOOT.len(), |at| from + at);
    &SHELL_BOOT[from..to]
}

/// A page that is the window's own, as far as a webview the backend does not
/// know sees it: Tauri's `window.__TAURI__.core.invoke` (a `fetch` of
/// `ipc://localhost/<command>`, refused), the window's fault reporter, and a
/// boot that leaves `boot_calls` refused calls uncaught. `fixed` picks the
/// reporter the window has now; otherwise it is the one it had. The page stops
/// asking after `STORM_MS` so that a run ends.
fn storm_page(fixed: bool, boot_calls: u32) -> String {
    let reporter = if fixed {
        fault_reporter_now()
    } else {
        FAULT_REPORTER_BEFORE
    };
    format!(
        "<!doctype html><meta charset=\"utf-8\"><script>\
const STORM_UNTIL = performance.now() + {STORM_MS};\
window.__TAURI__ = {{ core: {{ invoke: (command, args) => performance.now() > STORM_UNTIL ? Promise.resolve(null) : fetch('ipc://localhost/' + command, {{ method: 'POST', headers: {{ 'Content-Type': 'application/json' }}, body: JSON.stringify(args ?? {{}}) }}).then(async (reply) => {{ const text = await reply.text(); if (!reply.ok) throw text; return text; }}) }} }};\
{reporter}\
for (let at = 0; at < {boot_calls}; at += 1) window.__TAURI__.core.invoke('boot_phase', {{ phase: 'p' + at }});\
</script><p>storm</p>"
    )
}

/// What a webview the backend does not know is told, on the spot and on the
/// main thread, the way Tauri refuses a command it does not allow it: a 400
/// that says so, with the header that lets the page read it.
fn storm_road(request: Request<Vec<u8>>, responder: RequestAsyncResponder) {
    let preflight = *request.method() == Method::OPTIONS;
    SERVING.with(|serving| {
        *serving.borrow_mut() = Serving {
            body: request.body().len(),
            preflight,
            control: false,
            call_ns: 0,
        };
    });
    if preflight {
        responder.respond(reply(Vec::new(), "text/plain", true));
        return;
    }
    let command = request.uri().path().trim_start_matches('/');
    REFUSED.with(|held| *held.borrow_mut().entry(command.to_string()).or_default() += 1);
    responder.respond(
        Response::builder()
            .status(400)
            .header("Access-Control-Allow-Origin", "*")
            .header(CONTENT_TYPE, "text/plain")
            .body(format!("Command {command} not allowed by ACL").into_bytes())
            .unwrap(),
    );
}

/// What the run loop did over a stretch, from its turn-to-turn gaps: a loop
/// asked to wake every millisecond that stood still for a long gap is a main
/// thread that was busy — or blocked — that long.
fn gap_stats(mut gaps: Vec<u64>) -> Value {
    gaps.sort_unstable();
    let over = |ms: u64| gaps.iter().filter(|gap| **gap >= ms * 1_000_000).count();
    let stalled_ms: u64 = gaps
        .iter()
        .filter(|gap| **gap >= 16_000_000)
        .map(|gap| gap / 1_000_000)
        .sum();
    json!({
        "turns": gaps.len(),
        "p95_ms": microseconds(percentile(&gaps, 0.95)) / 1000.0,
        "max_ms": microseconds(gaps.last().copied().unwrap_or(0)) / 1000.0,
        "over_16ms": over(16),
        "over_100ms": over(100),
        "over_1000ms": over(1000),
        "stalled_ms": stalled_ms,
    })
}

fn take_gaps() -> Vec<u64> {
    PHASE
        .lock()
        .unwrap()
        .take()
        .map(|phase| phase.gaps_ns)
        .unwrap_or_default()
}

/// Makes the child webview the way the thumbnail pane is made: in the parent
/// window's view tree, parked outside it (or hidden after it is made).
fn born(window: &tao::window::Window, page: &str, place: &str) -> Result<WebView, String> {
    let html = match page {
        "busy" => BUSY_PAGE.to_string(),
        "heavy" => heavy_page(),
        "layers" => layers_page(),
        _ => CALM_PAGE.to_string(),
    };
    WebViewBuilder::new()
        .with_bounds(Rect {
            position: LogicalPosition::new(CHILD_PARKED_AT, CHILD_PARKED_AT).into(),
            size: LogicalSize::new(CHILD_SIZE.0, CHILD_SIZE.1).into(),
        })
        .with_html(html)
        .with_on_page_load_handler(|event, url| {
            if matches!(event, PageLoadEvent::Finished) {
                *LOADED_AT.lock().unwrap() = Some((Instant::now(), url));
            }
        })
        .build_as_child(window)
        .map(|webview| {
            if place == "hidden" {
                let _ = webview.set_visible(false);
            }
            webview
        })
        .map_err(|error| error.to_string())
}

fn reply(body: Vec<u8>, content_type: &str, preflight: bool) -> Response<Vec<u8>> {
    let mut response = Response::builder()
        .status(200)
        .header("Access-Control-Allow-Origin", "*")
        .header(CONTENT_TYPE, content_type);
    if preflight {
        response = response.header("Access-Control-Allow-Headers", "*");
    } else {
        response = response
            .header("Access-Control-Expose-Headers", "Tauri-Response")
            .header("Tauri-Response", "ok");
    }
    response.body(body).unwrap()
}

/// What a Tauri command costs the calling thread before the command's own
/// thread has it: the JSON body parsed into a `Value` (or the bytes kept as
/// they are), and the reply handed to another thread — or sent on the spot,
/// the way a command that is not `async` answers.
fn tauri_like(request: &Request<Vec<u8>>, responder: RequestAsyncResponder) {
    let content_type = request
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("");
    if content_type.starts_with("application/json") && !request.body().is_empty() {
        let value = serde_json::from_slice::<Value>(request.body());
        std::hint::black_box(&value);
    }
    let response = reply(b"null".to_vec(), "application/json", false);
    if SYNC_REPLY.load(Ordering::Relaxed) {
        responder.respond(response);
        return;
    }
    let sender = REPLIES.get().unwrap().lock().unwrap();
    let _ = sender.send((responder, response));
}

/// The page talking to the tool: opening and closing a phase, switching the
/// reply mode, asking for an `evaluate_script` reading, finishing.
fn control(request: &Request<Vec<u8>>) -> Value {
    let text = String::from_utf8_lossy(request.body()).to_string();
    match request.uri().path() {
        "/__phase" => {
            open_phase(&text);
            Value::Null
        }
        "/__phase_end" => {
            let wall = text.parse::<f64>().unwrap_or(0.0);
            let result = close_phase(wall);
            eprintln!("{result}");
            RESULTS.lock().unwrap().push(result);
            Value::Null
        }
        "/__sync" => {
            SYNC_REPLY.store(text == "1", Ordering::Relaxed);
            Value::Null
        }
        "/__eval" => {
            let ask: Value = serde_json::from_str(&text).unwrap_or(Value::Null);
            let bytes = ask["bytes"].as_u64().unwrap_or(0) as usize;
            let reps = ask["reps"].as_u64().unwrap_or(1) as usize;
            let result = eval_phase(bytes, reps);
            eprintln!("{result}");
            RESULTS.lock().unwrap().push(result);
            Value::Null
        }
        "/__error" => {
            eprintln!("page error: {text}");
            Value::Null
        }
        "/__done" => {
            finish(0);
            Value::Null
        }
        _ => Value::Null,
    }
}

/// Writes what was measured and tells the event loop to stop.
fn finish(code: i32) {
    let results = RESULTS.lock().unwrap().clone();
    let document = json!({
        "wry": "0.55.1",
        "tao": "0.35.3",
        "exit_code": code,
        "phases": results,
    });
    if let Some(path) = OUT.get() {
        let _ = std::fs::write(path, serde_json::to_vec_pretty(&document).unwrap());
    }
    EXIT_CODE.store(code, Ordering::Relaxed);
    DONE.store(true, Ordering::Relaxed);
}

fn ipc_road(request: Request<Vec<u8>>, responder: RequestAsyncResponder) {
    let preflight = *request.method() == Method::OPTIONS;
    let control_path = request.uri().path().starts_with(CONTROL);
    SERVING.with(|serving| {
        *serving.borrow_mut() = Serving {
            body: request.body().len(),
            preflight,
            control: control_path,
            call_ns: 0,
        };
    });
    if preflight {
        responder.respond(reply(Vec::new(), "text/plain", true));
    } else if control_path {
        control(&request);
        responder.respond(reply(b"null".to_vec(), "application/json", false));
    } else {
        tauri_like(&request, responder);
    }
}

fn page_road(_request: Request<Vec<u8>>, responder: RequestAsyncResponder) {
    SERVING.with(|serving| {
        *serving.borrow_mut() = Serving {
            control: true,
            ..Serving::default()
        };
    });
    let page = if BIRTH_MODE.load(Ordering::Relaxed) {
        BIRTH_MAIN_PAGE
    } else {
        PAGE
    };
    responder.respond(
        Response::builder()
            .header(CONTENT_TYPE, "text/html")
            .body(page.as_bytes().to_vec())
            .unwrap(),
    );
}

fn main() {
    let mut out = None;
    let mut quick = false;
    let mut birth: Option<String> = None;
    let mut thumb_spec: Option<String> = None;
    let mut assert_stall = false;
    let mut limit = DEFAULT_LIMIT_SECS;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--out" => out = args.next(),
            "--quick" => quick = true,
            "--birth" => birth = args.next(),
            "--thumb" => thumb_spec = args.next(),
            "--assert-stall" => assert_stall = true,
            "--ui-page" => {
                if let Some(path) = args.next() {
                    let _ = UI_PAGE.set(path);
                }
            }
            "--limit-secs" => limit = args.next().and_then(|n| n.parse().ok()).unwrap_or(limit),
            other => {
                eprintln!("unknown argument: {other}");
                std::process::exit(2);
            }
        }
    }
    let Some(out) = out else {
        eprintln!(
            "usage: ipc-road --out <result.json> [--quick] [--limit-secs <n>] [--birth <calm|busy|heavy|layers>:<parked|hidden|none>] [--thumb <calm|busy|heavy|layers|storm[N]|stormfix[N]|ui|uicut>[:store][:window][:poke][:hold<ms>] [--assert-stall] [--ui-page <index.html>]]"
        );
        std::process::exit(2);
    };
    OUT.set(out).unwrap();
    tracing::subscriber::set_global_default(Road).expect("one subscriber");

    // The replies leave on a thread of their own, as an async command's do.
    let (sender, receiver) = channel::<(RequestAsyncResponder, Response<Vec<u8>>)>();
    REPLIES.set(Mutex::new(sender)).ok();
    std::thread::spawn(move || {
        for (responder, response) in receiver {
            responder.respond(response);
        }
    });
    // A run that never finishes says what it has and stops.
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(limit));
        eprintln!("ipc-road: gave up after {limit} s");
        finish(3);
        std::thread::sleep(Duration::from_millis(500));
        std::process::exit(3);
    });

    let mut event_loop = EventLoop::new();
    event_loop.set_activation_policy(ActivationPolicy::Accessory);
    let own_window = birth.is_some() || thumb_spec.is_some();
    BIRTH_MODE.store(own_window, Ordering::Relaxed);
    let window = WindowBuilder::new()
        .with_title("ipc-road")
        // The birth mode needs a window AppKit treats as shown, so it stands
        // outside every screen instead of staying hidden.
        .with_visible(own_window)
        .with_position(LogicalPosition::new(PARENT_AT, 100.0))
        .with_inner_size(LogicalSize::new(400.0, 300.0))
        .build(&event_loop)
        .expect("a window");
    let url = if quick {
        "tauri://localhost/index.html?quick=1"
    } else {
        "tauri://localhost/index.html"
    };
    let webview = WebViewBuilder::new()
        .with_asynchronous_custom_protocol("tauri".into(), |_id, request, responder| {
            page_road(request, responder)
        })
        .with_asynchronous_custom_protocol("ipc".into(), |_id, request, responder| {
            ipc_road(request, responder)
        })
        .with_url(url)
        .build(&window)
        .expect("a webview");
    WEBVIEW.with(|held| *held.borrow_mut() = Some(webview));

    let mut last_wake: Option<Instant> = None;
    let begun = Instant::now();
    let mut birth_stage = 0_u8;
    let mut child: Option<WebView> = None;
    let mut birth_call_ms = 0.0_f64;
    let mut birth_at: Option<Instant> = None;
    let mut idle_gaps = Value::Null;
    let mut thumb = thumb_spec.as_deref().map(thumb::Thumb::new);
    event_loop.run(move |event, target, control_flow| {
        let now = Instant::now();
        *control_flow = ControlFlow::WaitUntil(now + WAKE_EVERY);
        if let Some(machine) = &mut thumb
            && now.duration_since(begun) >= thumb::WARM_UP
            && machine.step(&window, target, now)
        {
            thumb = None;
            finish(if assert_stall { thumb::verdict() } else { 0 });
        }
        if let Some(wanted) = &birth {
            let since = now.duration_since(begun);
            if birth_stage == 0 && since >= Duration::from_millis(500) {
                open_phase("idle");
                birth_stage = 1;
            } else if birth_stage == 1 && since >= Duration::from_millis(500) + BIRTH_BEFORE {
                idle_gaps = gap_stats(take_gaps());
                let (page, place) = wanted
                    .split_once(':')
                    .unwrap_or((wanted.as_str(), "parked"));
                open_phase("after");
                birth_at = Some(Instant::now());
                if place != "none" {
                    match born(&window, page, place) {
                        Ok(webview) => child = Some(webview),
                        Err(error) => eprintln!("birth failed: {error}"),
                    }
                }
                birth_call_ms = birth_at.map_or(0.0, |at| at.elapsed().as_secs_f64() * 1000.0);
                birth_stage = 2;
            } else if birth_stage == 2
                && since >= Duration::from_millis(500) + BIRTH_BEFORE + BIRTH_AFTER
            {
                let after = gap_stats(take_gaps());
                let loaded_ms = LOADED_AT
                    .lock()
                    .unwrap()
                    .as_ref()
                    .zip(birth_at)
                    .map(|((loaded, _), at)| loaded.duration_since(at).as_secs_f64() * 1000.0);
                let result = json!({
                    "phase": format!("birth-{wanted}"),
                    "birth_call_ms": (birth_call_ms * 10.0).round() / 10.0,
                    "load_finished_ms": loaded_ms.map(|ms| (ms * 10.0).round() / 10.0),
                    "idle": idle_gaps,
                    "after": after,
                });
                eprintln!("{result}");
                RESULTS.lock().unwrap().push(result);
                drop(child.take());
                birth_stage = 3;
                finish(0);
            }
        }
        if let Event::NewEvents(_) = event {
            if let Some(last) = last_wake {
                let gap = u64::try_from(now.duration_since(last).as_nanos()).unwrap_or(u64::MAX);
                if let Some(phase) = PHASE.lock().unwrap().as_mut() {
                    phase.gaps_ns.push(gap);
                }
            }
            last_wake = Some(now);
        }
        if DONE.load(Ordering::Relaxed) {
            *control_flow = ControlFlow::ExitWithCode(EXIT_CODE.load(Ordering::Relaxed));
        }
        // The window lives as long as the loop does.
        let _ = &window;
    });
}
