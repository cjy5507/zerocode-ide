//! Phase 0, the GPUI half: a recording's frames drawn by a GPUI terminal grid,
//! the same frames the window's DOM terminal is fed (`phase0/dom-replay.mjs`).
//!
//!   phase0-gpui timing  --recordings DIR --theme dom-theme.json --out DIR [--cases a,b] [--repeat N] [--level L]
//!                       [--window-kind normal|popup] [--since-ms EPOCH_MS] [--runner-pid PID]
//!   phase0-gpui capture --recordings DIR --theme dom-theme.json --out DIR [--cases a,b]
//!
//! timing — a window on the screen, not focused, one frame per display frame:
//! at each frame signal the next frame of the recording goes into the screen
//! model and the view is notified; GPUI then draws and presents in the same
//! turn. GPUI's own frame journal (`gpui::profiler`) says when the frame
//! became dirty, when its draw started and ended (and how long layout,
//! prepaint and paint took), and when its submission to the platform ended.
//! Submission is CPU time: the GPU's execution and the display's scanout are
//! not in it. After the recording, the unchanged screen is drawn again for a
//! while (the cost of a frame that changes nothing, which GPUI pays in full),
//! then frames pass with nothing drawn (the frame supply).
//!
//! GPUI stops a window's frames while macOS reports it not visible (covered,
//! minimised, hidden, on another Space, a sleeping display), so a pass only
//! counts while its window is seen. `--window-kind popup` opens the window as
//! GPUI's pop-up panel: above ordinary windows, on every Space, never taking
//! the keyboard; its title says it is a measurement window that closes by
//! itself. The pass records each visibility change GPUI reports and feeds its
//! first frame only once GPUI has reported the window visible (frames come
//! before that report: the pop-up opens reported hidden and turns visible some
//! 50–75 ms later). It ends at once, its samples written and its window
//! closed, when the window is reported hidden after the first fed frame or is
//! not reported visible within 5 s of opening (exit 6), when a keyboard, mouse
//! or trackpad event comes after `--since-ms` (exit 5), when the process
//! `--runner-pid` names is gone (exit 7), or when no frame comes for 5 s (exit 4).
//!
//! capture — a window that is never shown. At each checkpoint the screen is
//! drawn on demand and read back with `Window::render_to_image`, beside what
//! the painter drew (each glyph's text, columns, ink and face; each ground
//! run). Nothing is timed in this pass.
//!
//! Both passes stop before drawing when the window the platform gave cannot
//! hold the whole grid (macOS keeps a titled window inside the screen's visible
//! frame): a frame drawn in part is neither timed nor pictured, and the
//! refusal says how many rows and columns would fit.

mod grid;
mod model;
mod theme;

use std::borrow::Cow;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::rc::Rc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::profiler::{FrameEvent, FrameTimingCollector, set_trace_enabled};
use gpui::{
    AnyWindowHandle, App, AppContext, Bounds, Context, Entity, EntityId, IntoElement, Render,
    TitlebarOptions, Window, WindowBounds, WindowKind, WindowOptions, px, size,
};
use serde_json::{Value, json};
use zerocode_pty::GridDelta;

use grid::{Painter, Record, TerminalGrid};
use model::Screen;
use theme::Theme;

const USAGE: &str = "usage: phase0-gpui timing|capture --recordings DIR --theme dom-theme.json --out DIR [--cases a,b] [--repeat N] [--level L] [--font FAMILY] [--dom-fonts dom-fonts.json] [--font-file PATH] [--redraw N] [--idle N] [--window-kind normal|popup] [--since-ms EPOCH_MS] [--runner-pid PID]";

/// No frame signal for this long means the window is not being drawn — covered, minimised or on a sleeping display.
const STALL: Duration = Duration::from_secs(5);

/// The timing window's title. As a pop-up it covers most of a 14-inch screen for about half a minute.
const TIMING_TITLE: &str = "ZeroCode 측정 창 — 자동으로 닫힙니다";

/// How a timing pass that did not finish ended, by exit code.
const EXIT_STALLED: i32 = 4;
const EXIT_INPUT: i32 = 5;
const EXIT_HIDDEN: i32 = 6;
const EXIT_RUNNER_GONE: i32 = 7;

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    /// Seconds since the last event of `event_type` (`u32::MAX`: any input) in the
    /// event source state `state` (1: the HID system's, the one `HIDIdleTime` counts).
    fn CGEventSourceSecondsSinceLastEventType(state: i32, event_type: u32) -> f64;
}

unsafe extern "C" {
    /// POSIX `kill`; signal 0 only asks whether the process exists.
    fn kill(pid: i32, signal: i32) -> i32;
}

/// Seconds since the last keyboard, mouse or trackpad event the HID system saw.
#[cfg(target_os = "macos")]
fn seconds_since_input() -> f64 {
    // SAFETY: a query of the window server's event counters; it takes and returns plain numbers.
    unsafe { CGEventSourceSecondsSinceLastEventType(1, u32::MAX) }
}

#[cfg(not(target_os = "macos"))]
fn seconds_since_input() -> f64 {
    f64::INFINITY
}

fn epoch_ms() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs_f64() * 1000.0)
        .unwrap_or(0.0)
}

/// Milliseconds after `since_ms` (epoch) of the latest input event, if one came after it.
fn input_after(since_ms: f64) -> Option<f64> {
    let last = epoch_ms() - seconds_since_input() * 1000.0;
    (last > since_ms).then_some(last - since_ms)
}

fn process_alive(pid: i32) -> bool {
    // SAFETY: signal 0 delivers nothing; it only checks that `pid` exists.
    unsafe { kill(pid, 0) == 0 }
}

#[derive(Clone)]
struct Args {
    mode: String,
    recordings: PathBuf,
    theme: PathBuf,
    out: PathBuf,
    cases: Vec<String>,
    repeat: usize,
    level: String,
    font: String,
    dom_fonts: Option<PathBuf>,
    font_files: Vec<PathBuf>,
    /// What `choose_font` settled on: the family asked for and how it was found.
    font_facts: Value,
    redraw: usize,
    idle: usize,
    /// `normal` or `popup` (GPUI's pop-up panel: above ordinary windows, on every Space).
    window_kind: String,
    /// Epoch milliseconds the round began at; any input after it ends the pass.
    since_ms: Option<f64>,
    /// The runner's process: when it is gone, the pass ends and its window closes.
    runner_pid: Option<i32>,
}

fn parse() -> Result<Args, String> {
    let mut raw = std::env::args().skip(1);
    let mode = raw.next().ok_or(USAGE)?;
    if mode != "timing" && mode != "capture" {
        return Err(USAGE.into());
    }
    let mut args = Args {
        mode,
        recordings: PathBuf::new(),
        theme: PathBuf::new(),
        out: PathBuf::new(),
        cases: Vec::new(),
        repeat: 1,
        level: "normal".into(),
        font: "SF Mono".into(),
        dom_fonts: None,
        font_files: Vec::new(),
        font_facts: Value::Null,
        redraw: 120,
        idle: 120,
        window_kind: "normal".into(),
        since_ms: None,
        runner_pid: None,
    };
    while let Some(flag) = raw.next() {
        let mut value = || raw.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--recordings" => args.recordings = PathBuf::from(value()?),
            "--theme" => args.theme = PathBuf::from(value()?),
            "--out" => args.out = PathBuf::from(value()?),
            "--cases" => {
                args.cases = value()?
                    .split(',')
                    .filter(|c| !c.is_empty())
                    .map(String::from)
                    .collect()
            }
            "--repeat" => args.repeat = value()?.parse().map_err(|e| format!("--repeat: {e}"))?,
            "--level" => args.level = value()?,
            "--font" => args.font = value()?,
            "--dom-fonts" => args.dom_fonts = Some(PathBuf::from(value()?)),
            "--font-file" => args.font_files.push(PathBuf::from(value()?)),
            "--redraw" => args.redraw = value()?.parse().map_err(|e| format!("--redraw: {e}"))?,
            "--idle" => args.idle = value()?.parse().map_err(|e| format!("--idle: {e}"))?,
            "--window-kind" => {
                args.window_kind = value()?;
                if args.window_kind != "normal" && args.window_kind != "popup" {
                    return Err(format!(
                        "--window-kind: normal or popup, not {}",
                        args.window_kind
                    ));
                }
            }
            "--since-ms" => {
                args.since_ms = Some(value()?.parse().map_err(|e| format!("--since-ms: {e}"))?)
            }
            "--runner-pid" => {
                let pid: i32 = value()?.parse().map_err(|e| format!("--runner-pid: {e}"))?;
                if pid <= 0 {
                    return Err(format!("--runner-pid: a process id, not {pid}"));
                }
                args.runner_pid = Some(pid);
            }
            other => return Err(format!("unknown flag {other}\n{USAGE}")),
        }
    }
    if args.recordings.as_os_str().is_empty()
        || args.theme.as_os_str().is_empty()
        || args.out.as_os_str().is_empty()
    {
        return Err(USAGE.into());
    }
    Ok(args)
}

struct Recording {
    name: String,
    rows: usize,
    cols: usize,
    checkpoints: Vec<usize>,
    deltas: Vec<GridDelta>,
}

fn load(dir: &Path, wanted: &[String]) -> Result<Vec<Recording>, String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .map_err(|e| format!("{}: {e}", dir.display()))?
        .filter_map(|entry| entry.ok()?.file_name().into_string().ok())
        .filter_map(|name| name.strip_suffix(".meta.json").map(String::from))
        .collect();
    names.sort();
    let picked: Vec<String> = if wanted.is_empty() {
        names.clone()
    } else {
        wanted.to_vec()
    };
    let mut out = Vec::new();
    for name in picked {
        if !names.contains(&name) {
            return Err(format!("no recording {name} in {}", dir.display()));
        }
        let meta: Value = serde_json::from_str(
            &fs::read_to_string(dir.join(format!("{name}.meta.json")))
                .map_err(|e| format!("{name}: {e}"))?,
        )
        .map_err(|e| format!("{name}.meta.json: {e}"))?;
        let text = fs::read_to_string(dir.join(format!("{name}.deltas.jsonl")))
            .map_err(|e| format!("{name}: {e}"))?;
        let deltas = text
            .lines()
            .filter(|line| !line.is_empty())
            .enumerate()
            .map(|(at, line)| {
                serde_json::from_str::<GridDelta>(line)
                    .map_err(|e| format!("{name} frame {at}: {e}"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        out.push(Recording {
            rows: meta["rows"].as_u64().unwrap_or(0) as usize,
            cols: meta["cols"].as_u64().unwrap_or(0) as usize,
            checkpoints: meta["checkpoints"]
                .as_array()
                .map(|all| {
                    all.iter()
                        .filter_map(Value::as_u64)
                        .map(|k| k as usize)
                        .collect()
                })
                .unwrap_or_default(),
            deltas,
            name,
        });
    }
    Ok(out)
}

/// The plain CPU loop of tools/low-spec/spin.mjs: how much slower this level makes this process's own thread.
fn calibrate() -> f64 {
    let started = Instant::now();
    let mut x: i32 = 0;
    for i in 0..300_000_000_i32 {
        x = std::hint::black_box(x.wrapping_mul(31).wrapping_add(i));
    }
    std::hint::black_box(x);
    started.elapsed().as_secs_f64() * 1000.0
}

fn ms(duration: Duration) -> f64 {
    duration.as_secs_f64() * 1000.0
}

fn since(later: Instant, earlier: Instant) -> f64 {
    ms(later.saturating_duration_since(earlier))
}

fn stats(values: impl Iterator<Item = f64>) -> Value {
    let mut sorted: Vec<f64> = values.filter(|v| v.is_finite()).collect();
    if sorted.is_empty() {
        return Value::Null;
    }
    sorted.sort_by(f64::total_cmp);
    let at =
        |p: f64| sorted[((sorted.len() as f64 * p).ceil() as usize).clamp(1, sorted.len()) - 1];
    let round = |v: f64| (v * 1000.0).round() / 1000.0;
    json!({ "n": sorted.len(), "p50": round(at(0.5)), "p95": round(at(0.95)), "p99": round(at(0.99)), "max": round(sorted[sorted.len() - 1]) })
}

/// This process's resident memory, read by `ps` on a thread of its own every quarter second.
fn rss_sampler() -> Arc<Mutex<Vec<(Instant, u64)>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let into = Arc::clone(&seen);
    let pid = std::process::id().to_string();
    std::thread::spawn(move || {
        loop {
            if let Ok(out) = Command::new("ps").args(["-o", "rss=", "-p", &pid]).output()
                && let Ok(kib) = String::from_utf8_lossy(&out.stdout).trim().parse::<u64>()
                && let Ok(mut seen) = into.lock()
            {
                seen.push((Instant::now(), kib));
            }
            std::thread::sleep(Duration::from_millis(250));
        }
    });
    seen
}

fn power() -> Value {
    let read = |cmd: &str, args: &[&str]| {
        Command::new(cmd)
            .args(args)
            .output()
            .map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
            .unwrap_or_default()
    };
    let load = read("sysctl", &["-n", "vm.loadavg"]);
    let batt = read("pmset", &["-g", "batt"]);
    json!({
        "loadavg": load.trim(),
        "source": if batt.contains("AC Power") { "AC" } else if batt.contains("Battery Power") { "battery" } else { "unknown" },
        "cores": std::thread::available_parallelism().map(|n| n.get()).unwrap_or(0),
    })
}

struct Probe {
    screen: Rc<RefCell<Screen>>,
    painter: Rc<RefCell<Painter>>,
}

impl Render for Probe {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        TerminalGrid {
            screen: Rc::clone(&self.screen),
            painter: Rc::clone(&self.painter),
        }
    }
}

fn window_size(theme: &Theme, recordings: &[Recording]) -> gpui::Size<gpui::Pixels> {
    let rows = recordings.iter().map(|r| r.rows).max().unwrap_or(24);
    let cols = recordings.iter().map(|r| r.cols).max().unwrap_or(80);
    size(
        px(cols as f32 * theme.cell_width),
        px(rows as f32 * theme.cell_height),
    )
}

/// Whether the window the platform gave holds the whole grid. macOS keeps a
/// titled window inside its screen's visible frame (the screen less the menu
/// bar and the Dock), so on a small display a grid can be drawn only in part:
/// GPUI culls what falls outside the window, and a picture of it is cut off.
/// Neither pass draws such a grid; the refusal says what would fit.
struct GridFit {
    rows: usize,
    cols: usize,
    width: f32,
    height: f32,
    viewport: (f32, f32),
}

impl GridFit {
    fn of(theme: &Theme, recordings: &[Recording], viewport: gpui::Size<gpui::Pixels>) -> Self {
        let need = window_size(theme, recordings);
        Self {
            rows: recordings.iter().map(|r| r.rows).max().unwrap_or(24),
            cols: recordings.iter().map(|r| r.cols).max().unwrap_or(80),
            width: need.width.as_f32(),
            height: need.height.as_f32(),
            viewport: (viewport.width.as_f32(), viewport.height.as_f32()),
        }
    }

    fn fits(&self) -> bool {
        self.width <= self.viewport.0 + 0.5 && self.height <= self.viewport.1 + 0.5
    }

    fn most(&self, theme: &Theme) -> (usize, usize) {
        (
            ((self.viewport.1 + 0.5) / theme.cell_height).floor() as usize,
            ((self.viewport.0 + 0.5) / theme.cell_width).floor() as usize,
        )
    }

    fn to_json(&self, theme: &Theme) -> Value {
        let (rows, cols) = self.most(theme);
        json!({
            "rows": self.rows, "cols": self.cols,
            "width": self.width, "height": self.height,
            "viewport": [self.viewport.0, self.viewport.1],
            "fits": self.fits(),
            "mostRows": rows, "mostCols": cols,
        })
    }

    fn refuse_unless_whole(&self, theme: &Theme, out: &Path, mode: &str) {
        if self.fits() {
            return;
        }
        let (rows, cols) = self.most(theme);
        fail(
            out,
            mode,
            &format!(
                "the window holds {:.0} × {:.0} points and the grid needs {:.0} × {:.0} ({} rows × {} columns): \
                 macOS keeps a titled window inside the screen's visible frame, so part of the grid would not be drawn. \
                 Record at most {rows} rows × {cols} columns (phase0-record cases --rows R --cols C) or use a larger display",
                self.viewport.0, self.viewport.1, self.width, self.height, self.rows, self.cols
            ),
        );
    }
}

fn fail(out: &Path, mode: &str, why: &str) -> ! {
    let _ = fs::create_dir_all(out);
    let _ = fs::write(
        out.join(format!("gpui-{mode}-error.json")),
        json!({ "error": why }).to_string(),
    );
    eprintln!("phase0-gpui: {why}");
    std::process::exit(3);
}

// ---------------------------------------------------------------- timing

#[derive(Default, Clone)]
struct Sample {
    k: usize,
    kind: &'static str,
    tick_at: Option<Instant>,
    gap_ms: Option<f64>,
    apply_ms: Option<f64>,
    notified: bool,
    dirty_at: Option<Instant>,
    signal_at: Option<Instant>,
    draw_start: Option<Instant>,
    draw_end: Option<Instant>,
    phases: Option<(f64, f64, f64)>,
    invalidations: u64,
    present_start: Option<Instant>,
    present_end: Option<Instant>,
    glyphs: usize,
    shaped: usize,
    grounds: usize,
    /// The next frame's tick, once it came: a draw that started before it was
    /// drawn in the turn this sample was fed in.
    next_tick: Option<Instant>,
}

impl Sample {
    /// Drawn in the turn it was fed: the draw started after this sample's
    /// tick and before the next one, not a display frame later.
    fn same_turn(&self) -> bool {
        match (self.draw_start, self.tick_at) {
            (Some(start), Some(tick)) => {
                start >= tick && self.next_tick.is_none_or(|next| start < next)
            }
            _ => false,
        }
    }

    fn json(&self) -> Value {
        let tick = self.tick_at;
        let after_tick = |at: Option<Instant>| match (at, tick) {
            (Some(at), Some(tick)) => Some(since(at, tick)),
            _ => None,
        };
        json!({
            "k": self.k, "kind": self.kind, "gapMs": self.gap_ms, "applyMs": self.apply_ms,
            "notified": self.notified,
            "dirtyAfterTickMs": after_tick(self.dirty_at),
            "drawStartAfterTickMs": after_tick(self.draw_start),
            "feedToDrawEndMs": after_tick(self.draw_end),
            "feedToPresentEndMs": after_tick(self.present_end),
            "drawMs": match (self.draw_start, self.draw_end) { (Some(s), Some(e)) => Some(since(e, s)), _ => None },
            "requestLayoutMs": self.phases.map(|p| p.0), "prepaintMs": self.phases.map(|p| p.1), "paintMs": self.phases.map(|p| p.2),
            "presentMs": match (self.present_start, self.present_end) { (Some(s), Some(e)) => Some(since(e, s)), _ => None },
            "invalidations": self.invalidations,
            "glyphs": self.glyphs, "shaped": self.shaped, "grounds": self.grounds,
            "sameTurn": self.same_turn(),
        })
    }
}

enum Phase {
    Feed(usize),
    Redraw(usize),
    Idle(usize),
}

struct Job {
    case: usize,
    round: usize,
}

struct Running {
    job: Job,
    phase: Phase,
    samples: Vec<Sample>,
    started: Instant,
    power_before: Value,
    /// macOS's thermal state as the case began (`cx.thermal_state()`).
    thermal_before: String,
}

/// Each visibility change GPUI reported for the timing window, with when it was delivered.
/// GPUI delivers them after the update in progress and folds a burst into its last value,
/// so a change inside one turn may leave no entry.
type VisibilityLog = Rc<RefCell<Vec<(Instant, bool)>>>;

struct Driver {
    args: Args,
    recordings: Vec<Recording>,
    jobs: VecDeque<Job>,
    running: Option<Running>,
    results: Vec<Value>,
    collector: FrameTimingCollector,
    view: Option<EntityId>,
    screen: Rc<RefCell<Screen>>,
    painter: Rc<RefCell<Painter>>,
    last_tick: Option<Instant>,
    rss: Arc<Mutex<Vec<(Instant, u64)>>>,
    calibration_ms: f64,
    window_facts: Value,
    stalled: bool,
    finished: bool,
    opened: Instant,
    visibility: VisibilityLog,
    /// When the first recording frame was fed: a hidden window after it ends the pass.
    first_feed_at: Option<Instant>,
}

impl Driver {
    /// Frame events since the last turn, each joined to the sample it drew.
    fn absorb(&mut self) {
        let events = self.collector.collect_unseen();
        let Some(running) = &mut self.running else {
            return;
        };
        for event in events {
            match event {
                FrameEvent::Draw(timing) => {
                    let Some(sample) = running.samples.iter_mut().rev().find(|sample| {
                        sample.notified
                            && sample.draw_start.is_none()
                            && sample.tick_at.is_some_and(|tick| timing.draw_start >= tick)
                    }) else {
                        continue;
                    };
                    sample.dirty_at = timing.dirty_at;
                    sample.signal_at = timing.signal_at;
                    sample.draw_start = Some(timing.draw_start);
                    sample.draw_end = Some(timing.draw_end);
                    sample.invalidations = timing.invalidations;
                    sample.phases = Some((
                        ms(timing.phases.request_layout),
                        ms(timing.phases.prepaint),
                        ms(timing.phases.paint),
                    ));
                }
                FrameEvent::Present(timing) => {
                    if let Some(sample) = running.samples.iter_mut().rev().find(|sample| {
                        sample.present_start.is_none()
                            && sample
                                .draw_end
                                .is_some_and(|end| timing.present_start >= end)
                    }) {
                        sample.present_start = Some(timing.present_start);
                        sample.present_end = Some(timing.present_end);
                    }
                }
            }
        }
    }

    /// A case's result: `aborted` names why a case that did not run to its end stopped.
    fn finish(&mut self, running: Running, thermal_after: Option<String>, aborted: Option<&str>) {
        let recording = &self.recordings[running.job.case];
        let ended = Instant::now();
        let seen: Vec<Value> = self
            .visibility
            .borrow()
            .iter()
            .filter(|(at, _)| *at >= running.started && *at <= ended)
            .map(|(at, visible)| json!({ "atMs": since(*at, running.started), "visible": visible }))
            .collect();
        let hidden = seen.iter().any(|event| event["visible"] == false);
        let rss: Vec<u64> = self
            .rss
            .lock()
            .map(|seen| {
                seen.iter()
                    .filter(|(at, _)| *at >= running.started && *at <= ended)
                    .map(|(_, kib)| *kib)
                    .collect()
            })
            .unwrap_or_default();
        let fed: Vec<&Sample> = running
            .samples
            .iter()
            .filter(|s| s.kind == "feed")
            .collect();
        // Frame 0 is the reader's floor, a full frame onto an empty grid; kept raw, left out of the steady numbers.
        let steady: Vec<&Sample> = fed.iter().skip(1).copied().collect();
        let redraw: Vec<&Sample> = running
            .samples
            .iter()
            .filter(|s| s.kind == "redraw")
            .collect();
        let idle: Vec<&Sample> = running
            .samples
            .iter()
            .filter(|s| s.kind == "idle")
            .collect();
        let field = |samples: &[&Sample], pick: &dyn Fn(&Sample) -> Option<f64>| {
            stats(samples.iter().filter_map(|s| pick(s)))
        };
        let rss_median = {
            let mut sorted = rss.clone();
            sorted.sort_unstable();
            sorted.get(sorted.len() / 2).copied()
        };
        let raw: Vec<Value> = running.samples.iter().map(Sample::json).collect();
        let to = |s: &Sample, at: Option<Instant>| match (at, s.tick_at) {
            (Some(at), Some(t)) => Some(since(at, t)),
            _ => None,
        };
        let draw = |s: &Sample| match (s.draw_start, s.draw_end) {
            (Some(a), Some(b)) => Some(since(b, a)),
            _ => None,
        };
        let present = |s: &Sample| match (s.present_start, s.present_end) {
            (Some(a), Some(b)) => Some(since(b, a)),
            _ => None,
        };
        let summary = json!({
            "feedToDrawEndMs": field(&steady, &|s| to(s, s.draw_end)),
            "feedToPresentEndMs": field(&steady, &|s| to(s, s.present_end)),
            "applyMs": field(&steady, &|s| s.apply_ms),
            "drawMs": field(&steady, &draw),
            "paintMs": field(&steady, &|s| s.phases.map(|p| p.2)),
            "prepaintMs": field(&steady, &|s| s.phases.map(|p| p.1)),
            "requestLayoutMs": field(&steady, &|s| s.phases.map(|p| p.0)),
            "presentMs": field(&steady, &present),
            "gapMs": field(&steady, &|s| s.gap_ms),
            "redrawFeedToDrawEndMs": field(&redraw, &|s| to(s, s.draw_end)),
            "redrawDrawMs": field(&redraw, &draw),
            "idleGapMs": field(&idle, &|s| s.gap_ms),
            "notDrawn": steady.iter().filter(|s| s.draw_end.is_none()).count(),
            "notSameTurn": steady.iter().filter(|s| !s.same_turn()).count(),
            "glyphsPerFrame": field(&steady, &|s| Some(s.glyphs as f64)),
            "rssPeakKiB": rss.iter().max(),
            "rssMedianKiB": rss_median,
        });
        println!(
            "gpui {} {} #{}: frames {}, feed→draw end p95 {} ms, feed→present end p95 {} ms, not drawn {}{}{}",
            self.args.level,
            recording.name,
            running.job.round,
            fed.len(),
            summary["feedToDrawEndMs"]["p95"],
            summary["feedToPresentEndMs"]["p95"],
            summary["notDrawn"],
            if hidden {
                ", hidden during the case"
            } else {
                ""
            },
            aborted
                .map(|why| format!(", ended early: {why}"))
                .unwrap_or_default(),
        );
        self.results.push(json!({
            "renderer": "gpui", "case": recording.name, "round": running.job.round, "level": self.args.level,
            "frames": fed.len(), "summary": summary,
            "complete": aborted.is_none(), "aborted": aborted,
            "hidden": hidden, "visibility": seen,
            "thermal": { "before": running.thermal_before, "after": thermal_after },
            "before": running.power_before, "after": power(),
            "samples": raw,
        }));
    }

    /// The pass's file. `ended` is how a pass that did not finish ended: a label
    /// (`stalled`, `input`, `hidden`, `runner`, `unsupported`) and what was seen.
    fn write(&self, ended: Option<(&str, String)>) {
        let visibility: Vec<Value> = self
            .visibility
            .borrow()
            .iter()
            .map(|(at, visible)| json!({ "atMs": since(*at, self.opened), "visible": visible }))
            .collect();
        let out = json!({
            "renderer": "gpui",
            "engine": "gpui-pre 0.3.12 (Zed d2a4fb6), Metal, rustc 1.98.1",
            "level": self.args.level,
            "font": self.args.font_facts,
            "window": self.window_facts,
            "calibration": { "ms": self.calibration_ms },
            // Kept under its first name: any pass that did not finish says why here.
            "stalled": ended.as_ref().map(|(_, why)| why.clone()),
            "aborted": ended.as_ref().map(|(label, _)| *label),
            "sinceMs": self.args.since_ms,
            "visibility": visibility,
            "firstFeedMs": self.first_feed_at.map(|at| since(at, self.opened)),
            "results": self.results,
        });
        let _ = fs::create_dir_all(&self.args.out);
        let path = self
            .args
            .out
            .join(format!("gpui-timing-{}.json", self.args.level));
        if let Err(error) = fs::write(&path, out.to_string()) {
            eprintln!("phase0-gpui: {}: {error}", path.display());
        }
    }
}

fn tick(driver: &Rc<RefCell<Driver>>, window: &mut Window, cx: &mut App) {
    let now = Instant::now();
    let mut d = driver.borrow_mut();
    if d.stalled {
        return;
    }
    // Nothing is fed until GPUI has reported the window visible: frames come before
    // that report does (the pop-up opens reported hidden and turns visible 50–75 ms on).
    let seen = d
        .visibility
        .borrow()
        .last()
        .is_some_and(|(_, visible)| *visible);
    if d.first_feed_at.is_none() && !seen {
        // No case runs yet, so the frame events since the last turn are dropped.
        d.absorb();
        d.last_tick = Some(now);
        drop(d);
        let again = Rc::clone(driver);
        window.on_next_frame(move |window, cx| tick(&again, window, cx));
        return;
    }
    d.absorb();
    let gap = d.last_tick.map(|last| since(now, last));
    d.last_tick = Some(now);
    loop {
        if d.running.is_none() {
            let Some(job) = d.jobs.pop_front() else {
                d.write(None);
                d.finished = true;
                drop(d);
                cx.quit();
                return;
            };
            d.running = Some(Running {
                job,
                phase: Phase::Feed(0),
                samples: Vec::new(),
                started: now,
                power_before: power(),
                thermal_before: format!("{:?}", cx.thermal_state()),
            });
        }
        let redraw = d.args.redraw;
        let idle = d.args.idle;
        let case = d.running.as_ref().map(|r| r.job.case).unwrap_or(0);
        let total = d.recordings[case].deltas.len();
        let running = d.running.as_mut().expect("a job runs");
        let next = match running.phase {
            Phase::Feed(k) if k >= total => Some(Phase::Redraw(0)),
            Phase::Redraw(n) if n >= redraw => Some(Phase::Idle(0)),
            Phase::Idle(n) if n >= idle => None,
            _ => break,
        };
        match next {
            Some(phase) => running.phase = phase,
            None => {
                let done = d.running.take().expect("a job runs");
                d.finish(done, Some(format!("{:?}", cx.thermal_state())), None);
            }
        }
    }
    let view = d.view.expect("the view stands");
    let mut sample = Sample {
        tick_at: Some(now),
        gap_ms: gap,
        ..Sample::default()
    };
    let case = d.running.as_ref().map(|r| r.job.case).unwrap_or(0);
    let phase = match d.running.as_ref().map(|r| &r.phase) {
        Some(Phase::Feed(k)) => Phase::Feed(*k),
        Some(Phase::Redraw(n)) => Phase::Redraw(*n),
        Some(Phase::Idle(n)) => Phase::Idle(*n),
        None => Phase::Idle(0),
    };
    match phase {
        Phase::Feed(k) => {
            sample.k = k;
            sample.kind = "feed";
            if d.first_feed_at.is_none() {
                d.first_feed_at = Some(now);
            }
            let applied = {
                let started = Instant::now();
                let result = d.screen.borrow_mut().apply(&d.recordings[case].deltas[k]);
                (result, since(Instant::now(), started))
            };
            if let Err(unsupported) = applied.0 {
                let why = format!("{} frame {k}: {}", d.recordings[case].name, unsupported.0);
                d.write(Some(("unsupported", why.clone())));
                fail(&d.args.out, "timing", &why);
            }
            sample.apply_ms = Some(applied.1);
            sample.notified = true;
            cx.notify(view);
        }
        Phase::Redraw(n) => {
            sample.k = n;
            sample.kind = "redraw";
            sample.notified = true;
            cx.notify(view);
        }
        Phase::Idle(n) => {
            sample.k = n;
            sample.kind = "idle";
        }
    }
    // The previous frame's paint counts belong to the sample before this one.
    let count = d.painter.borrow().count;
    if let Some(running) = d.running.as_mut() {
        if let Some(previous) = running.samples.last_mut() {
            previous.next_tick = Some(now);
            if previous.notified {
                previous.glyphs = count.glyphs;
                previous.shaped = count.shaped;
                previous.grounds = count.grounds;
            }
        }
        running.phase = match running.phase {
            Phase::Feed(k) => Phase::Feed(k + 1),
            Phase::Redraw(n) => Phase::Redraw(n + 1),
            Phase::Idle(n) => Phase::Idle(n + 1),
        };
        running.samples.push(sample);
    }
    drop(d);
    let again = Rc::clone(driver);
    window.on_next_frame(move |window, cx| tick(&again, window, cx));
}

fn start_timing(
    cx: &mut App,
    args: Args,
    recordings: Vec<Recording>,
    theme: Theme,
    calibration_ms: f64,
) {
    set_trace_enabled(true);
    let bounds = Bounds::centered(None, window_size(&theme, &recordings), cx);
    let screen = Rc::new(RefCell::new(Screen::new(
        recordings[0].rows,
        recordings[0].cols,
    )));
    let painter = Rc::new(RefCell::new(Painter::new(
        theme.clone(),
        args.font.clone(),
        true,
    )));
    let mut jobs = VecDeque::new();
    for round in 1..=args.repeat.max(1) {
        for case in 0..recordings.len() {
            jobs.push_back(Job { case, round });
        }
    }
    let log: VisibilityLog = Rc::new(RefCell::new(Vec::new()));
    let driver = Rc::new(RefCell::new(Driver {
        args: args.clone(),
        recordings,
        jobs,
        running: None,
        results: Vec::new(),
        collector: FrameTimingCollector::new(),
        view: None,
        screen: Rc::clone(&screen),
        painter: Rc::clone(&painter),
        last_tick: None,
        rss: rss_sampler(),
        calibration_ms,
        window_facts: Value::Null,
        stalled: false,
        finished: false,
        opened: Instant::now(),
        visibility: Rc::clone(&log),
        first_feed_at: None,
    }));
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        // The notice sits in an opaque, system-drawn title bar above the grid.
        titlebar: Some(TitlebarOptions {
            title: Some(TIMING_TITLE.into()),
            appears_transparent: false,
            traffic_light_position: None,
        }),
        // GPUI's pop-up is a panel above ordinary windows and on every Space, so the
        // person's windows cannot cover it; like the normal window, it never activates the app.
        kind: if args.window_kind == "popup" {
            WindowKind::PopUp
        } else {
            WindowKind::Normal
        },
        is_resizable: false,
        is_minimizable: false,
        show: true,
        // Never the person's keyboard: the window opens behind nothing it takes.
        focus: false,
        // An unfocused window is held to ~30 fps by default; a pane being read is the active one.
        inactive_frame_interval: None,
        ..Default::default()
    };
    let opened = cx.open_window(options, |_, cx| {
        cx.new(|_| Probe {
            screen: Rc::clone(&screen),
            painter: Rc::clone(&painter),
        })
    });
    let handle = match opened {
        Ok(handle) => handle,
        Err(error) => fail(
            &args.out,
            "timing",
            &format!("open the GPUI window: {error}"),
        ),
    };
    let observed = Rc::clone(&log);
    let facts = handle.update(cx, |_, window, cx| {
        let visible = window.is_visible();
        observed.borrow_mut().push((Instant::now(), visible));
        let changes = Rc::clone(&observed);
        window
            .observe_window_visibility(move |visibility, _, _| {
                changes
                    .borrow_mut()
                    .push((Instant::now(), visibility.is_visible()));
            })
            .detach();
        (
            window.viewport_size(),
            json!({
                "kind": args.window_kind,
                "title": TIMING_TITLE,
                "visibleAtOpen": visible,
                "scale": window.scale_factor(),
                "viewport": [window.viewport_size().width.as_f32(), window.viewport_size().height.as_f32()],
                "asked": [bounds.size.width.as_f32(), bounds.size.height.as_f32()],
                "gpu": format!("{:?}", window.gpu_specs()),
                "powerMode": format!("{:?}", cx.power_mode()),
                "thermal": format!("{:?}", cx.thermal_state()),
            }),
        )
    });
    let facts = match facts {
        Ok((viewport, mut facts)) => {
            // A frame that draws part of the grid is not a terminal frame: no sample is taken.
            let fit = GridFit::of(&theme, &driver.borrow().recordings, viewport);
            facts["grid"] = fit.to_json(&theme);
            fit.refuse_unless_whole(&theme, &args.out, "timing");
            facts
        }
        Err(error) => fail(
            &args.out,
            "timing",
            &format!("read the GPUI window: {error}"),
        ),
    };
    {
        let mut d = driver.borrow_mut();
        d.view = handle
            .root(cx)
            .ok()
            .map(|entity: Entity<Probe>| entity.entity_id());
        d.window_facts = facts;
        d.opened = Instant::now();
    }
    let first = Rc::clone(&driver);
    let _ = handle.update(cx, |_, window, _| {
        window.on_next_frame(move |window, cx| tick(&first, window, cx));
    });
    // Twice a second, outside any frame: a pass that can no longer be a clean one ends at
    // once, its samples written; the process exits and its window closes with it.
    let watch = Rc::clone(&driver);
    let since_ms = args.since_ms;
    let runner = args.runner_pid;
    cx.spawn(async move |cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let verdict = {
                let d = watch.borrow();
                if d.finished {
                    break;
                }
                let hidden = d.first_feed_at.and_then(|first| {
                    d.visibility
                        .borrow()
                        .iter()
                        .find(|(at, visible)| !*visible && *at >= first)
                        .map(|(at, _)| since(*at, first))
                });
                let silent = d.last_tick.unwrap_or(d.opened).elapsed();
                if let Some(after) = since_ms.and_then(input_after) {
                    Some((
                        EXIT_INPUT,
                        "input",
                        format!(
                            "a keyboard, mouse or trackpad event {after:.0} ms after the round began: the round is broken; the samples so far are written and the window closes"
                        ),
                    ))
                } else if let Some(after) = hidden {
                    Some((
                        EXIT_HIDDEN,
                        "hidden",
                        format!(
                            "the window was reported not visible {after:.0} ms after the first fed frame (covered, minimised, hidden, on another Space or a sleeping display): the samples so far are written and the window closes"
                        ),
                    ))
                } else if runner.is_some_and(|pid| !process_alive(pid)) {
                    Some((
                        EXIT_RUNNER_GONE,
                        "runner",
                        "the runner that started this pass is gone: the samples so far are written and the window closes".to_string(),
                    ))
                } else if d.first_feed_at.is_none() && d.opened.elapsed() > STALL {
                    Some((
                        EXIT_HIDDEN,
                        "hidden",
                        format!(
                            "the window was not reported visible within {} s of opening: nothing was fed",
                            STALL.as_secs()
                        ),
                    ))
                } else if silent > STALL {
                    Some((
                        EXIT_STALLED,
                        "stalled",
                        format!(
                            "no frame signal for {} s: the window is not being drawn (occluded, minimised or the display asleep); the samples so far are written",
                            STALL.as_secs()
                        ),
                    ))
                } else {
                    None
                }
            };
            let Some((code, label, why)) = verdict else {
                continue;
            };
            let mut d = watch.borrow_mut();
            d.stalled = true;
            if let Some(running) = d.running.take() {
                d.finish(running, None, Some(label));
            }
            d.write(Some((label, why.clone())));
            eprintln!("phase0-gpui: {why}");
            std::process::exit(code);
        }
    })
    .detach();
}

// ---------------------------------------------------------------- capture

fn run_capture(cx: &mut App, args: &Args, recordings: &[Recording], theme: Theme) {
    let screen = Rc::new(RefCell::new(Screen::new(
        recordings[0].rows,
        recordings[0].cols,
    )));
    let painter = Rc::new(RefCell::new(Painter::new(
        theme.clone(),
        args.font.clone(),
        false,
    )));
    let bounds = Bounds::centered(None, window_size(&theme, recordings), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        show: false,
        focus: false,
        ..Default::default()
    };
    let handle = match cx.open_window(options, |_, cx| {
        cx.new(|_| Probe {
            screen: Rc::clone(&screen),
            painter: Rc::clone(&painter),
        })
    }) {
        Ok(handle) => handle,
        Err(error) => fail(
            &args.out,
            "capture",
            &format!("open the GPUI window: {error}"),
        ),
    };
    let view = match handle.root(cx) {
        Ok(entity) => entity.entity_id(),
        Err(error) => fail(
            &args.out,
            "capture",
            &format!("read the GPUI view: {error}"),
        ),
    };
    let any_handle: AnyWindowHandle = handle.into();
    if let Err(error) = fs::create_dir_all(&args.out) {
        fail(
            &args.out,
            "capture",
            &format!("{}: {error}", args.out.display()),
        );
    }
    // A picture of part of the grid would count every cell past the window's edge as lost.
    let fit = match cx.update_window(any_handle, |_, window, _| window.viewport_size()) {
        Ok(viewport) => GridFit::of(&theme, recordings, viewport),
        Err(error) => fail(
            &args.out,
            "capture",
            &format!("read the GPUI window: {error}"),
        ),
    };
    fit.refuse_unless_whole(&theme, &args.out, "capture");
    let mut index = Vec::new();
    for recording in recordings {
        let mut states = Vec::new();
        for (k, delta) in recording.deltas.iter().enumerate() {
            if let Err(unsupported) = screen.borrow_mut().apply(delta) {
                fail(
                    &args.out,
                    "capture",
                    &format!("{} frame {k}: {}", recording.name, unsupported.0),
                );
            }
            if !recording.checkpoints.contains(&k) {
                continue;
            }
            painter.borrow_mut().record = Some(Record::default());
            // Through the window, not the view: drawing renders the root view,
            // which must not be borrowed by the caller while it does.
            let drawn = cx.update_window(any_handle, |_, window, cx| {
                cx.notify(view);
                window.refresh();
                let clear = window.draw(cx);
                clear.clear(cx);
                let image = window.render_to_image();
                let quads = window.painted_quads().len();
                let underlines = window.painted_underlines().len();
                (
                    image,
                    quads,
                    underlines,
                    window.scale_factor(),
                    window.viewport_size(),
                )
            });
            let (image, quads, underlines, scale, viewport) = match drawn {
                Ok(drawn) => drawn,
                Err(error) => fail(
                    &args.out,
                    "capture",
                    &format!("draw {} frame {k}: {error}", recording.name),
                ),
            };
            let png = args.out.join(format!("{}.{k:04}.gpui.png", recording.name));
            match image {
                Ok(image) => {
                    if let Err(error) = image.save(&png) {
                        fail(&args.out, "capture", &format!("{}: {error}", png.display()));
                    }
                }
                Err(error) => fail(
                    &args.out,
                    "capture",
                    &format!("render_to_image {} frame {k}: {error}", recording.name),
                ),
            }
            let record = painter.borrow_mut().record.take().unwrap_or_default();
            // The same frame drawn again with the caret, as the timing pass draws it: the
            // cells that differ from the picture above are where the caret was painted —
            // the caret seen in pixels, beside the model's cursor.
            let cursor_visible = screen.borrow().cursor_visible;
            let cursor_png = cursor_visible.then(|| {
                args.out
                    .join(format!("{}.{k:04}.cursor.gpui.png", recording.name))
            });
            if let Some(path) = &cursor_png {
                painter.borrow_mut().draw_cursor = true;
                let drawn = cx.update_window(any_handle, |_, window, cx| {
                    cx.notify(view);
                    window.refresh();
                    let clear = window.draw(cx);
                    clear.clear(cx);
                    window.render_to_image()
                });
                painter.borrow_mut().draw_cursor = false;
                match drawn {
                    Ok(Ok(image)) => {
                        if let Err(error) = image.save(path) {
                            fail(
                                &args.out,
                                "capture",
                                &format!("{}: {error}", path.display()),
                            );
                        }
                    }
                    Ok(Err(error)) | Err(error) => fail(
                        &args.out,
                        "capture",
                        &format!("draw the caret {} frame {k}: {error}", recording.name),
                    ),
                }
            }
            let model = screen.borrow();
            let (grid_w, grid_h) = (
                model.cols as f32 * theme.cell_width,
                model.rows as f32 * theme.cell_height,
            );
            states.push(json!({
                "frame": k,
                "png": png,
                "cursorPng": cursor_png,
                "scale": scale,
                "viewport": [viewport.width.as_f32(), viewport.height.as_f32()],
                // The grid's own bounds in the window, in points: it is the window's root element.
                "gridBounds": { "x": 0.0, "y": 0.0, "width": grid_w, "height": grid_h,
                                "fits": grid_w <= viewport.width.as_f32() + 0.5 && grid_h <= viewport.height.as_f32() + 0.5 },
                "grid": { "rows": model.rows, "cols": model.cols, "cellW": theme.cell_width, "cellH": theme.cell_height },
                "cursor": { "row": model.cursor.0, "col": model.cursor.1, "visible": model.cursor_visible },
                "sceneQuads": quads,
                "sceneUnderlines": underlines,
                "painted": record.to_json(),
            }));
        }
        let file = args
            .out
            .join(format!("{}.gpui-capture.json", recording.name));
        if let Err(error) = fs::write(&file, json!({ "renderer": "gpui", "case": recording.name, "frames": recording.deltas.len(), "states": states }).to_string()) {
            fail(&args.out, "capture", &format!("{}: {error}", file.display()));
        }
        println!(
            "gpui capture {}: {} checkpoints",
            recording.name,
            states.len()
        );
        index.push(json!({ "case": recording.name, "file": file, "checkpoints": states.len() }));
    }
    let _ = fs::write(
        args.out.join("gpui-capture-index.json"),
        json!({ "engine": "gpui-pre 0.3.12 (Zed d2a4fb6), Metal, rustc 1.98.1", "font": args.font_facts, "grid": fit.to_json(&theme), "index": index }).to_string(),
    );
    cx.quit();
}

/// A family macOS keeps for its own use cannot be picked by name (`SF Mono` is
/// not among the families an app sees), but its file can be loaded.
fn system_font_file(family: &str) -> Option<PathBuf> {
    let file = match family {
        "SF Mono" => "/System/Library/Fonts/SFNSMono.ttf",
        _ => return None,
    };
    Some(PathBuf::from(file)).filter(|path| path.exists())
}

/// The family the grid paints with: the one the page drew its ASCII runs in
/// (`dom-fonts.json`, read from the browser), else `--font`; loaded from its
/// file when the system does not offer it by name. What was found is written
/// beside every result, so a run in a fallback face says so.
fn choose_font(cx: &mut App, args: &Args) -> (String, Value) {
    let mut family = args.font.clone();
    let mut from = "--font";
    if let Some(path) = &args.dom_fonts
        && let Ok(text) = fs::read_to_string(path)
        && let Ok(json) = serde_json::from_str::<Value>(&text)
        && let Some(primary) = json["primary"].as_str()
    {
        family = primary.to_string();
        from = "dom-fonts.json";
    }
    let offered = |cx: &App, family: &str| {
        cx.text_system()
            .all_font_names()
            .iter()
            .any(|name| name == family)
    };
    let offered_by_name = offered(&*cx, &family);
    let mut files = args.font_files.clone();
    if !offered_by_name && let Some(file) = system_font_file(&family) {
        files.push(file);
    }
    let mut loaded = Vec::new();
    for file in files {
        match fs::read(&file) {
            Ok(bytes) => match cx.text_system().add_fonts(vec![Cow::Owned(bytes)]) {
                Ok(()) => loaded.push(json!({ "file": file, "ok": true })),
                Err(error) => {
                    loaded.push(json!({ "file": file, "ok": false, "error": error.to_string() }))
                }
            },
            Err(error) => {
                loaded.push(json!({ "file": file, "ok": false, "error": error.to_string() }))
            }
        }
    }
    let available = offered(&*cx, &family);
    if !available {
        eprintln!(
            "phase0-gpui: the family {family:?} is not available; GPUI falls back to another face"
        );
    }
    let facts = json!({ "family": family, "from": from, "offeredByName": offered_by_name, "loaded": loaded, "available": available });
    (family, facts)
}

fn main() {
    let args = match parse() {
        Ok(args) => args,
        Err(why) => {
            eprintln!("{why}");
            std::process::exit(2);
        }
    };
    let theme = Theme::load(&args.theme).unwrap_or_else(|why| fail(&args.out, &args.mode, &why));
    let recordings =
        load(&args.recordings, &args.cases).unwrap_or_else(|why| fail(&args.out, &args.mode, &why));
    if recordings.is_empty() {
        fail(&args.out, &args.mode, "no recordings");
    }
    let calibration_ms = if args.mode == "timing" {
        calibrate()
    } else {
        0.0
    };
    gpui_platform::application().run(move |cx: &mut App| {
        let mut args = args;
        let (family, facts) = choose_font(cx, &args);
        args.font = family;
        args.font_facts = facts;
        if args.mode == "timing" {
            start_timing(cx, args, recordings, theme, calibration_ms);
        } else {
            run_capture(cx, &args, &recordings, theme);
        }
    });
}
