//! The artifact gallery's thumbnail pane, run the way the window runs it
//! (t-20972).
//!
//! `artifact_thumbs::hidden_pane` makes ONE child webview the first time a
//! card asks, parks it far outside the window, on the default browser profile's
//! data store, and for every card loads the page by its `file://` address,
//! waits for `Finished`, lets one beat pass, takes a snapshot at card width with
//! `artifact_webkit::take_snapshot_png`, and sends the pane back to a blank page.
//! The hang of 2026-10-01 17:28 (7.4 s) began about half a second after the
//! first of those panes of its process was made and ended with the thumbnail it
//! wrote. This runs
//! the same steps three times — the first round includes the pane's birth — and
//! reports, for each step, how long the main thread's run loop stood still.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

use objc2::MainThreadMarker;
use serde_json::{Value, json};
use tao::event_loop::EventLoopWindowTarget;
use tao::window::{Window, WindowBuilder};
use wry::dpi::{LogicalPosition, LogicalSize};
use wry::{PageLoadEvent, Rect, WebView, WebViewBuilder, WebViewBuilderExtDarwin, WebViewExtMacOS};

use crate::{
    BUSY_PAGE, CALM_PAGE, LOADED_AT, NO_BRIDGE, SERVED, SERVED_NS, STORM_BOOT_CALLS, TAURI_SHIM,
    UI_PAGE, artifact_webkit, gap_stats, heavy_page, layers_page, open_phase, refused_since,
    refused_snapshot, storm_page, storm_road, take_gaps,
};

/// How long the run loop is watched before the first round begins.
pub(crate) const WARM_UP: Duration = Duration::from_millis(2000);
/// `zerocode_core::artifact::Limits::default()`: the viewport the pane is laid
/// out at (`thumb_viewport_width` 1024, 4:3), the width the snapshot is taken at
/// (`thumb_width`), and how long a page may take to finish (`thumb_timeout_ms`).
const VIEWPORT: (f64, f64) = (1024.0, 768.0);
const SNAPSHOT_WIDTH: f64 = 320.0;
const LOAD_TIMEOUT: Duration = Duration::from_millis(8_000);
/// `artifact_thumbs::LOAD_POLL`: the beat after `Finished` before the snapshot.
const SETTLE: Duration = Duration::from_millis(50);
/// `artifact_thumbs::PARKED_AT`.
const PARKED_AT: f64 = -20_000.0;
/// How long the pane is left on its blank page before the next round.
const BLANK_FOR: Duration = Duration::from_millis(300);
/// How long a snapshot may take.
const SNAPSHOT_TIMEOUT: Duration = Duration::from_secs(10);
/// How many cards are rendered.
const ROUNDS: u32 = 3;
/// How often `:poke` touches the main window.
const POKE_EVERY: Duration = Duration::from_millis(50);
/// Stands for the default browser profile's identifier (16 bytes).
const STORE: [u8; 16] = *b"ipc-road-thumbs!";
/// The longest the main thread's run loop may stand still in any one step of the
/// pane's road — its birth, its load, its snapshot, its return to the blank
/// page — before `--assert-stall` calls the run a stall: a quarter of a second,
/// the most a pointer may stick before a person sees it (fifteen frames at 60 Hz)
/// and ten times the 22 ms the run loop stood still with no pane at all
/// (`--birth none:none`, 2026-10-02). The watchdog's two seconds is where a
/// stall is called a hang, not where it starts to hurt.
pub(crate) const STALL_BOUND_MS: f64 = 250.0;

enum Stage {
    Begin,
    Loading,
    Settle,
    Snapshot,
    Blank,
}

#[derive(Default)]
struct Marks {
    birth_ms: f64,
    load_ms: Option<f64>,
    snapshot_call_ms: f64,
    snapshot_ms: Option<f64>,
    png_bytes: usize,
    /// How many requests `start_task` had served, and for how long, when the
    /// round began: the round's own share is the difference at its end.
    served_at_start: (u64, u64),
    /// What the pane's refusing `ipc` road had been asked for when the round began.
    refused_at_start: HashMap<String, u64>,
    gaps: Vec<(&'static str, Value)>,
}

pub(crate) struct Thumb {
    page: String,
    store: bool,
    /// The pane is the content of a window of its own, outside every screen,
    /// instead of a child of the main window.
    own_window: bool,
    holder: Option<Window>,
    /// The main window is resized by a pixel every `POKE_EVERY`: window-level
    /// activity (what a person's moving and resizing is) while a page loads.
    poke: bool,
    poked_at: Instant,
    poked: u32,
    url: String,
    round: u32,
    stage: Stage,
    since: Instant,
    pane: Option<WebView>,
    waiting: Option<Receiver<Result<Vec<u8>, String>>>,
    marks: Marks,
}

fn millis(duration: Duration) -> f64 {
    (duration.as_secs_f64() * 10_000.0).round() / 10.0
}

/// `storm` or `stormfix`, then optionally how many refused calls the page's boot
/// leaves uncaught (`storm32`): whether the page has the window's reporter as it
/// is now, and that number.
fn storm_spec(page: &str) -> Option<(bool, u32)> {
    let (fixed, digits) = match page.strip_prefix("stormfix") {
        Some(rest) => (true, rest),
        None => (false, page.strip_prefix("storm")?),
    };
    if digits.is_empty() {
        return Some((fixed, STORM_BOOT_CALLS));
    }
    digits.parse().ok().map(|calls| (fixed, calls))
}

/// The page files the pane loads: written once, loaded by `file://` like an
/// artifact page.
fn page_file(page: &str) -> String {
    // The window's own page, where it stands on disk: nothing is written.
    if (page == "ui" || page == "uicut")
        && let Some(path) = UI_PAGE.get()
    {
        return format!("file://{path}");
    }
    let dir = std::env::temp_dir().join("ipc-road-thumb");
    let _ = std::fs::create_dir_all(&dir);
    let html = match page {
        "busy" => BUSY_PAGE.to_string(),
        "heavy" => heavy_page(),
        "layers" => layers_page(),
        _ if storm_spec(page).is_some() => {
            let (fixed, boot_calls) = storm_spec(page).unwrap_or((false, STORM_BOOT_CALLS));
            storm_page(fixed, boot_calls)
        }
        _ => CALM_PAGE.to_string(),
    };
    let path = dir.join(format!("{page}.html"));
    let _ = std::fs::write(&path, html);
    format!("file://{}", path.display())
}

/// `artifact_thumbs::hidden_pane`, as far as wry shows it: a child of the
/// window, parked outside it, on a named data store — or, for the comparison,
/// the content of a window of its own that stands outside every screen.
fn born(
    window: &Window,
    target: &EventLoopWindowTarget<()>,
    store: bool,
    own_window: bool,
    refusing: bool,
    shim: bool,
    cut: bool,
) -> Result<(WebView, Option<Window>), String> {
    let holder = if own_window {
        Some(
            WindowBuilder::new()
                .with_title("ipc-road thumb")
                .with_decorations(false)
                .with_focused(false)
                .with_always_on_bottom(true)
                .with_visible(true)
                .with_position(LogicalPosition::new(PARKED_AT, PARKED_AT))
                .with_inner_size(LogicalSize::new(VIEWPORT.0, VIEWPORT.1))
                .build(target)
                .map_err(|error| error.to_string())?,
        )
    } else {
        None
    };
    let mut builder = WebViewBuilder::new()
        .with_bounds(Rect {
            position: LogicalPosition::new(PARKED_AT, PARKED_AT).into(),
            size: LogicalSize::new(VIEWPORT.0, VIEWPORT.1).into(),
        })
        .with_url("about:blank")
        .with_on_page_load_handler(|event, url| {
            if matches!(event, PageLoadEvent::Finished) {
                *LOADED_AT.lock().unwrap() = Some((Instant::now(), url));
            }
        });
    if store {
        builder = builder.with_data_store_identifier(STORE);
    }
    if shim {
        builder = builder.with_initialization_script(TAURI_SHIM);
    }
    if cut {
        // The window's own script for its hidden panes, after the bridge it takes away.
        builder = builder.with_initialization_script(NO_BRIDGE);
    }
    if refusing {
        // The pane's own `ipc` road: it refuses everything, as the backend does
        // a webview it does not know.
        builder = builder
            .with_asynchronous_custom_protocol("ipc".into(), |_id, request, responder| {
                storm_road(request, responder)
            });
    }
    let webview = match &holder {
        Some(own) => builder.build(own),
        None => builder.build_as_child(window),
    }
    .map_err(|error| error.to_string())?;
    Ok((webview, holder))
}

/// 0 when no step of any round stood still longer than `STALL_BOUND_MS`, else
/// 4 — after saying which step of which round did.
pub(crate) fn verdict() -> i32 {
    let results = crate::RESULTS.lock().unwrap();
    let mut worst: Option<(String, String, f64)> = None;
    for row in results.iter() {
        let Some(phase) = row["phase"]
            .as_str()
            .filter(|name| name.starts_with("thumb-"))
        else {
            continue;
        };
        let Some(steps) = row["steps"].as_object() else {
            continue;
        };
        for (step, gaps) in steps {
            let gap = gaps["max_ms"].as_f64().unwrap_or(0.0);
            if worst.as_ref().is_none_or(|(_, _, held)| gap > *held) {
                worst = Some((phase.to_string(), step.clone(), gap));
            }
        }
    }
    match worst {
        Some((phase, step, gap)) if gap > STALL_BOUND_MS => {
            eprintln!(
                "STALL {phase}: the run loop stood still {gap} ms in the {step} step (bound {STALL_BOUND_MS} ms)"
            );
            4
        }
        Some((phase, step, gap)) => {
            eprintln!(
                "no stall: the longest stand-still was {gap} ms ({phase}, {step} step; bound {STALL_BOUND_MS} ms)"
            );
            0
        }
        None => {
            eprintln!("no thumbnail round ran");
            4
        }
    }
}

impl Thumb {
    /// `spec` is `<page>` followed by any of `:store` (a named data store, as
    /// the window names one), `:window` (the pane in a window of its own) and
    /// `:poke` (the main window resized by a pixel every 50 ms meanwhile).
    pub(crate) fn new(spec: &str) -> Self {
        let mut parts = spec.split(':');
        let page = parts.next().unwrap_or("calm");
        let flags: Vec<&str> = parts.collect();
        Self {
            page: page.to_string(),
            store: flags.contains(&"store"),
            own_window: flags.contains(&"window"),
            holder: None,
            poke: flags.contains(&"poke"),
            poked_at: Instant::now(),
            poked: 0,
            url: page_file(page),
            round: 0,
            stage: Stage::Begin,
            since: Instant::now(),
            pane: None,
            waiting: None,
            marks: Marks::default(),
        }
    }

    /// One turn of the run loop; true when every round is done.
    pub(crate) fn step(
        &mut self,
        window: &Window,
        target: &EventLoopWindowTarget<()>,
        now: Instant,
    ) -> bool {
        if self.poke && now.duration_since(self.poked_at) >= POKE_EVERY {
            self.poked_at = now;
            self.poked += 1;
            window.set_inner_size(LogicalSize::new(400.0 + f64::from(self.poked % 2), 300.0));
        }
        match self.stage {
            Stage::Begin => {
                self.marks = Marks {
                    served_at_start: (
                        SERVED.load(Ordering::Relaxed),
                        SERVED_NS.load(Ordering::Relaxed),
                    ),
                    refused_at_start: refused_snapshot(),
                    ..Marks::default()
                };
                open_phase("birth");
                let started = Instant::now();
                if self.pane.is_none() {
                    match born(
                        window,
                        target,
                        self.store,
                        self.own_window,
                        self.page.starts_with("storm") || self.page.starts_with("ui"),
                        self.page.starts_with("ui"),
                        self.page == "uicut",
                    ) {
                        Ok((pane, holder)) => {
                            self.pane = Some(pane);
                            self.holder = holder;
                        }
                        Err(error) => {
                            eprintln!("thumb pane failed: {error}");
                            return true;
                        }
                    }
                }
                self.marks.birth_ms = millis(started.elapsed());
                self.marks.gaps.push(("birth", gap_stats(take_gaps())));
                *LOADED_AT.lock().unwrap() = None;
                open_phase("load");
                if let Some(pane) = &self.pane {
                    let _ = pane.load_url(&self.url);
                }
                self.since = Instant::now();
                self.stage = Stage::Loading;
            }
            Stage::Loading => {
                let loaded = LOADED_AT.lock().unwrap().clone();
                let finished = loaded.filter(|(_, url)| url != "about:blank");
                if let Some((at, _)) = finished {
                    self.marks.load_ms = Some(millis(at.duration_since(self.since)));
                } else if now.duration_since(self.since) <= LOAD_TIMEOUT {
                    return false;
                }
                self.marks.gaps.push(("load", gap_stats(take_gaps())));
                self.since = now;
                self.stage = Stage::Settle;
            }
            Stage::Settle => {
                if now.duration_since(self.since) < SETTLE {
                    return false;
                }
                open_phase("snapshot");
                let (sender, receiver) = channel();
                if let (Some(pane), Some(mtm)) = (&self.pane, MainThreadMarker::new()) {
                    let view = pane.webview();
                    let called = Instant::now();
                    artifact_webkit::take_snapshot_png(mtm, &view, Some(SNAPSHOT_WIDTH), sender);
                    self.marks.snapshot_call_ms = millis(called.elapsed());
                }
                self.waiting = Some(receiver);
                self.since = Instant::now();
                self.stage = Stage::Snapshot;
            }
            Stage::Snapshot => {
                let answer = self.waiting.as_ref().map(Receiver::try_recv);
                match answer {
                    Some(Ok(result)) => {
                        self.marks.snapshot_ms = Some(millis(self.since.elapsed()));
                        self.marks.png_bytes = result.map_or(0, |png| png.len());
                    }
                    Some(Err(TryRecvError::Empty))
                        if now.duration_since(self.since) <= SNAPSHOT_TIMEOUT =>
                    {
                        return false;
                    }
                    _ => {}
                }
                self.waiting = None;
                self.marks.gaps.push(("snapshot", gap_stats(take_gaps())));
                open_phase("blank");
                if let Some(pane) = &self.pane {
                    let _ = pane.load_url("about:blank");
                }
                self.since = now;
                self.stage = Stage::Blank;
            }
            Stage::Blank => {
                if now.duration_since(self.since) < BLANK_FOR {
                    return false;
                }
                self.marks.gaps.push(("blank", gap_stats(take_gaps())));
                self.round += 1;
                let worst = self
                    .marks
                    .gaps
                    .iter()
                    .filter_map(|(_, gaps)| gaps["max_ms"].as_f64())
                    .fold(0.0_f64, f64::max);
                let steps: serde_json::Map<String, Value> = self
                    .marks
                    .gaps
                    .iter()
                    .map(|(name, gaps)| ((*name).to_string(), gaps.clone()))
                    .collect();
                let label = format!(
                    "{}{}{}",
                    if self.store { "-store" } else { "" },
                    if self.own_window { "-window" } else { "" },
                    if self.poke { "-poke" } else { "" }
                );
                let (served_before, served_ns_before) = self.marks.served_at_start;
                let served = SERVED.load(Ordering::Relaxed).saturating_sub(served_before);
                let served_ns = SERVED_NS
                    .load(Ordering::Relaxed)
                    .saturating_sub(served_ns_before);
                let result = json!({
                    "phase": format!("thumb-{}{label}-round{}", self.page, self.round),
                    "requests": served,
                    "request_main_ms": (served_ns as f64 / 1e4).round() / 100.0,
                    "most_asked": refused_since(&self.marks.refused_at_start, 8),
                    "birth_ms": self.marks.birth_ms,
                    "load_ms": self.marks.load_ms,
                    "snapshot_call_ms": self.marks.snapshot_call_ms,
                    "snapshot_ms": self.marks.snapshot_ms,
                    "png_bytes": self.marks.png_bytes,
                    "worst_gap_ms": worst,
                    "steps": steps,
                });
                eprintln!("{result}");
                crate::RESULTS.lock().unwrap().push(result);
                if self.round >= ROUNDS {
                    return true;
                }
                self.stage = Stage::Begin;
            }
        }
        false
    }
}
