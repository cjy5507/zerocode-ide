//! The window's own record of how long it took to come up (t-20078).
//!
//! Every start writes one JSON file and one window-log line saying, in
//! milliseconds since the process began, when each boot phase was reached.
//! Nothing but phase names and numbers goes in: no paths, no titles, no
//! terminal text — the file is safe to attach to a report.
//!
//! The order of [`Phase::ORDER`] is the contract. A change that moves restore
//! or ledger work in front of the first paint shows up as a `first_paint`
//! number that grew, and as a test that goes red (`boot_timeline` tests plus
//! the source contract on `ui/shell.js`).

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// The file the timeline is kept in, under the window's local data root.
pub(crate) const TIMELINE_FILE: &str = "boot-timeline.json";

/// Why the file carries a version: a reader (the sampler, a report) must be
/// able to tell a layout it does not know from one it does.
const TIMELINE_VERSION: u32 = 1;

/// The window-log tag that makes the one summary line greppable.
const LOG_TAG: &str = "boot-timeline";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Phase {
    ChromiumReady,
    SettingsRead,
    LedgerReady,
    WebviewCreated,
    RendererUp,
    PageScriptLoaded,
    FirstPaint,
    StageRestored,
    LayoutsRead,
    PanesRestored,
    BrowserTabsRestored,
    FirstBrowserTab,
    TerminalsResumed,
}

impl Phase {
    /// The order a healthy boot reaches them in. Phases the window cannot
    /// reach in the same order (a restore that finds nothing to restore still
    /// reports) are still reported in this order.
    pub(crate) const ORDER: [Phase; 13] = [
        Phase::ChromiumReady,
        Phase::SettingsRead,
        Phase::LedgerReady,
        Phase::WebviewCreated,
        Phase::RendererUp,
        Phase::PageScriptLoaded,
        Phase::FirstPaint,
        Phase::StageRestored,
        Phase::LayoutsRead,
        Phase::PanesRestored,
        Phase::BrowserTabsRestored,
        Phase::FirstBrowserTab,
        Phase::TerminalsResumed,
    ];

    /// The phases the page reports through `boot_phase`; the rest are the
    /// backend's own.
    pub(crate) const FROM_PAGE: [Phase; 7] = [
        Phase::PageScriptLoaded,
        Phase::FirstPaint,
        Phase::StageRestored,
        Phase::LayoutsRead,
        Phase::PanesRestored,
        Phase::BrowserTabsRestored,
        Phase::TerminalsResumed,
    ];

    /// Where a phase stands in the chain. The browser tabs and the resumed
    /// terminals both follow the panes and neither follows the other: the
    /// resumes run behind the browser restore and finish whenever they do.
    pub(crate) fn rank(self) -> usize {
        match self {
            Phase::BrowserTabsRestored | Phase::TerminalsResumed => Phase::PanesRestored.rank() + 1,
            other => Phase::ORDER
                .iter()
                .position(|held| *held == other)
                .unwrap_or_default(),
        }
    }

    /// A phase a healthy boot may never reach: no browser tab was stored.
    pub(crate) fn optional(self) -> bool {
        self == Phase::FirstBrowserTab
    }

    pub(crate) fn key(self) -> &'static str {
        match self {
            Phase::ChromiumReady => "chromium_ready",
            Phase::SettingsRead => "settings_read",
            Phase::LedgerReady => "ledger_ready",
            Phase::WebviewCreated => "webview_created",
            Phase::RendererUp => "renderer_up",
            Phase::PageScriptLoaded => "page_script_loaded",
            Phase::FirstPaint => "first_paint",
            Phase::StageRestored => "stage_restored",
            Phase::LayoutsRead => "layouts_read",
            Phase::PanesRestored => "panes_restored",
            Phase::BrowserTabsRestored => "browser_tabs_restored",
            Phase::FirstBrowserTab => "first_browser_tab",
            Phase::TerminalsResumed => "terminals_resumed",
        }
    }

    pub(crate) fn from_page_key(key: &str) -> Option<Phase> {
        Phase::FROM_PAGE
            .into_iter()
            .find(|phase| phase.key() == key)
    }
}

/// What has been reached so far. Pure data, so the order and the shape of the
/// file can be pinned without a window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Timeline {
    reached: Vec<(Phase, u64)>,
}

impl Timeline {
    /// First report wins: a phase reached twice (the page reloads) keeps the
    /// number of the boot, not of the reload.
    pub(crate) fn record(&mut self, phase: Phase, at_ms: u64) -> bool {
        if self.at(phase).is_some() {
            return false;
        }
        self.reached.push((phase, at_ms));
        true
    }

    pub(crate) fn at(&self, phase: Phase) -> Option<u64> {
        self.reached
            .iter()
            .find(|(held, _)| *held == phase)
            .map(|(_, at_ms)| *at_ms)
    }

    pub(crate) fn complete(&self) -> bool {
        Phase::ORDER
            .iter()
            .filter(|phase| !phase.optional())
            .all(|phase| self.at(*phase).is_some())
    }

    /// The reached phases that came earlier than a phase that must precede
    /// them — work put back in front of something it should follow. Phases
    /// of equal [`Phase::rank`] may land in either order.
    pub(crate) fn out_of_order(&self) -> Vec<Phase> {
        Phase::ORDER
            .into_iter()
            .filter(|phase| {
                let Some(at_ms) = self.at(*phase) else {
                    return false;
                };
                Phase::ORDER.into_iter().any(|before| {
                    before.rank() < phase.rank()
                        && self.at(before).is_some_and(|reached| reached > at_ms)
                })
            })
            .collect()
    }

    pub(crate) fn to_json(&self) -> serde_json::Value {
        let mut phases = serde_json::Map::new();
        for phase in Phase::ORDER {
            phases.insert(
                phase.key().to_string(),
                self.at(phase)
                    .map_or(serde_json::Value::Null, serde_json::Value::from),
            );
        }
        serde_json::json!({
            "version": TIMELINE_VERSION,
            "app_version": env!("CARGO_PKG_VERSION"),
            "unit": "ms since process start",
            "complete": self.complete(),
            "phases": phases,
        })
    }

    pub(crate) fn log_line(&self) -> String {
        let mut line = String::from(LOG_TAG);
        for phase in Phase::ORDER {
            match self.at(phase) {
                Some(at_ms) => line.push_str(&format!(" {}={at_ms}", phase.key())),
                None => line.push_str(&format!(" {}=-", phase.key())),
            }
        }
        line
    }
}

struct Recorder {
    started: Instant,
    timeline: Mutex<Timeline>,
    root: OnceLock<PathBuf>,
}

fn recorder() -> &'static Recorder {
    static RECORDER: OnceLock<Recorder> = OnceLock::new();
    RECORDER.get_or_init(|| Recorder {
        started: Instant::now(),
        timeline: Mutex::new(Timeline::default()),
        root: OnceLock::new(),
    })
}

/// The first line of `main`: the clock starts here, so every number is the
/// time since the process began.
pub(crate) fn begin() {
    let _ = recorder();
}

/// Where the file and the log line go. Phases reached before this is known
/// are held in memory and written as soon as it is.
pub(crate) fn set_root(local_data_root: &Path) {
    let recorder = recorder();
    if recorder.root.set(local_data_root.to_path_buf()).is_ok() {
        let snapshot = recorder
            .timeline
            .lock()
            .unwrap_or_else(|held| held.into_inner())
            .clone();
        write_file(local_data_root, &snapshot);
    }
}

/// Reach a phase now. Cheap and safe from any thread; the file is rewritten
/// (a few hundred bytes) and the log line goes out once, when the last phase
/// lands.
pub(crate) fn mark(phase: Phase) {
    let recorder = recorder();
    let at_ms = u64::try_from(recorder.started.elapsed().as_millis()).unwrap_or(u64::MAX);
    let (fresh, snapshot) = {
        let mut held = recorder
            .timeline
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        let fresh = held.record(phase, at_ms);
        (fresh, held.clone())
    };
    if !fresh {
        return;
    }
    let Some(root) = recorder.root.get() else {
        return;
    };
    write_file(root, &snapshot);
    if snapshot.complete() {
        crate::note_window_event(root, &snapshot.log_line());
    }
}

fn write_file(root: &Path, timeline: &Timeline) {
    let Ok(bytes) = serde_json::to_vec(&timeline.to_json()) else {
        return;
    };
    let target = root.join(TIMELINE_FILE);
    let staged = root.join(format!("{TIMELINE_FILE}.tmp"));
    if std::fs::write(&staged, bytes).is_ok() {
        let _ = std::fs::rename(&staged, &target);
    }
}

/// The page reports its own phases here: it is the only side that knows when
/// its script ran, when it first painted and when its panes are back.
#[tauri::command(async)]
pub(crate) fn boot_phase(phase: String) -> Result<(), String> {
    let known = Phase::from_page_key(&phase)
        .ok_or_else(|| "that is not a phase the page reports".to_string())?;
    mark(known);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn full(offset: u64) -> Timeline {
        let mut timeline = Timeline::default();
        for (step, phase) in Phase::ORDER.into_iter().enumerate() {
            timeline.record(phase, offset + step as u64 * 10);
        }
        timeline
    }

    #[test]
    fn the_phases_are_these_in_this_order() {
        let keys: Vec<_> = Phase::ORDER.iter().map(|phase| phase.key()).collect();
        assert_eq!(
            keys,
            [
                "chromium_ready",
                "settings_read",
                "ledger_ready",
                "webview_created",
                "renderer_up",
                "page_script_loaded",
                "first_paint",
                "stage_restored",
                "layouts_read",
                "panes_restored",
                "browser_tabs_restored",
                "first_browser_tab",
                "terminals_resumed",
            ]
        );
    }

    #[test]
    fn the_first_paint_comes_before_any_restore() {
        let at = |phase: Phase| Phase::ORDER.iter().position(|held| *held == phase);
        assert!(at(Phase::FirstPaint) < at(Phase::PanesRestored));
        assert!(at(Phase::FirstPaint) < at(Phase::TerminalsResumed));
        assert!(at(Phase::PanesRestored) < at(Phase::TerminalsResumed));
    }

    #[test]
    fn the_browser_restore_and_the_resumes_may_finish_in_either_order() {
        let mut timeline = full(0);
        let resumed = timeline.at(Phase::TerminalsResumed).expect("resumed");
        let browsers = timeline.at(Phase::BrowserTabsRestored).expect("browsers");
        assert!(browsers < resumed);
        timeline
            .reached
            .retain(|(held, _)| *held != Phase::BrowserTabsRestored);
        timeline.record(Phase::BrowserTabsRestored, resumed + 500);
        assert!(timeline.out_of_order().is_empty());
    }

    #[test]
    fn a_healthy_boot_is_complete_and_in_order() {
        let timeline = full(5);
        assert!(timeline.complete());
        assert!(timeline.out_of_order().is_empty());
    }

    #[test]
    fn a_restore_that_finished_before_the_first_paint_is_named() {
        let mut timeline = Timeline::default();
        for (phase, at_ms) in [
            (Phase::SettingsRead, 10),
            (Phase::PanesRestored, 20),
            (Phase::FirstPaint, 30),
        ] {
            timeline.record(phase, at_ms);
        }
        assert_eq!(timeline.out_of_order(), vec![Phase::PanesRestored]);
    }

    #[test]
    fn the_first_report_of_a_phase_is_kept() {
        let mut timeline = Timeline::default();
        assert!(timeline.record(Phase::FirstPaint, 100));
        assert!(!timeline.record(Phase::FirstPaint, 9_000));
        assert_eq!(timeline.at(Phase::FirstPaint), Some(100));
    }

    #[test]
    fn the_file_holds_only_the_phase_names_and_numbers() {
        let json = full(0).to_json();
        let phases = json["phases"].as_object().expect("phases object");
        assert_eq!(phases.len(), Phase::ORDER.len());
        let top: Vec<_> = json.as_object().expect("object").keys().cloned().collect();
        for key in &top {
            assert!(
                ["version", "app_version", "unit", "complete", "phases"].contains(&key.as_str()),
                "unexpected key {key}"
            );
        }
        assert!(phases.values().all(serde_json::Value::is_number));
    }

    #[test]
    fn an_unreached_phase_is_null_in_the_file_and_a_dash_in_the_line() {
        let mut timeline = Timeline::default();
        timeline.record(Phase::SettingsRead, 7);
        assert!(timeline.to_json()["phases"]["first_paint"].is_null());
        assert!(timeline.log_line().contains("settings_read=7"));
        assert!(timeline.log_line().contains("first_paint=-"));
        assert!(!timeline.complete());
    }

    #[test]
    fn the_page_may_report_its_own_phases_and_no_others() {
        for phase in Phase::FROM_PAGE {
            assert_eq!(Phase::from_page_key(phase.key()), Some(phase));
        }
        assert_eq!(Phase::from_page_key("settings_read"), None);
        assert_eq!(Phase::from_page_key("/Users/somebody"), None);
    }

    #[test]
    fn the_file_is_written_where_it_is_told_and_the_line_is_the_log() {
        let root = std::env::temp_dir().join(format!("boot-timeline-{}", std::process::id()));
        std::fs::create_dir_all(&root).expect("root");
        write_file(&root, &full(1));
        let text = std::fs::read_to_string(root.join(TIMELINE_FILE)).expect("file");
        let json: serde_json::Value = serde_json::from_str(&text).expect("json");
        assert_eq!(json["complete"], true);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The page's side of the contract, read from its source: the marks are
    /// made in the order the timeline names, and the first paint is asked for
    /// before the restore is awaited — so work put back in front of the first
    /// paint goes red here and not only in a number on somebody's machine.
    #[test]
    fn the_page_marks_its_phases_in_order_and_paints_before_it_restores() {
        let shell = include_str!("../../../ui/shell.js");
        let boot = &shell[shell.find("async function boot() {").expect("boot")..];
        let mark_at = |needle: &str, from: usize| {
            from + boot[from..]
                .find(needle)
                .unwrap_or_else(|| panic!("boot never reaches {needle}"))
        };
        let paint = mark_at("await paintFirstFrame();", 0);
        let restore = mark_at("await restoreActiveWorktreeTab();", paint);
        let panes = mark_at("markBootPhase(\"panes_restored\");", restore);
        let browsers = mark_at("markBootPhase(\"browser_tabs_restored\");", panes);
        let resumed = mark_at("markBootPhase(\"terminals_resumed\")", panes);
        assert!(paint < restore && restore < panes && panes < browsers && panes < resumed);
        // The restore's own steps are marked where they happen.
        let status = include_str!("../../../ui/shell-status.js");
        let term = include_str!("../../../ui/shell-term.js");
        assert!(status.contains("markBootPhase(\"stage_restored\");"));
        assert!(term.contains("markBootPhase(\"layouts_read\");"));
        // No restore call may precede the paint mark inside `boot`.
        for restoring in ["restoreActiveWorktreeTab()", "restoreStandingWorkspaces()"] {
            assert!(
                boot.find(restoring).is_some_and(|at| at > paint),
                "{restoring} runs before the first paint is asked for"
            );
        }
        for phase in Phase::FROM_PAGE {
            assert!(
                shell.contains(&format!("markBootPhase(\"{}\")", phase.key()))
                    || status.contains(&format!("markBootPhase(\"{}\")", phase.key()))
                    || term.contains(&format!("markBootPhase(\"{}\")", phase.key())),
                "the page never reports {}",
                phase.key()
            );
        }
    }
}
