//! A census of the main window's native view tree — how many `NSView`s stand
//! under the content view, and how many `NSTrackingArea`s they register —
//! taken on the main thread between hang pings and cited by the hang report.
//!
//! Why it exists (2026-09-10): a 2,036 ms main-thread hang sampled inside
//! `mouseMoved` → `NSTrackingAreaAKManager updateActiveTrackingAreasForWindowLocation`
//! → `collectTrackingAreasForTargetAndWinLoc` walking an `NSArray`. AppKit
//! re-collects every tracking area of the window on every mouse move, so that
//! stack is O(N) in tracking areas — and the report carried no N. A census
//! that grows across a session (native subviews leaking: browser panes,
//! emulator mirrors, helpers) reads as the cause; a flat one rules it out.
//!
//! The walk runs on the main thread, after the watchdog's ping has been
//! answered, never inside the ping — a census must not lengthen the very
//! answer it exists to explain.

use std::fmt;
use std::sync::Mutex;
use std::time::Duration;

/// How often the census is taken. Thirty seconds: a leak that matters grows
/// over minutes, and one walk of a few thousand views is under a millisecond.
pub(crate) const CENSUS_EVERY: Duration = Duration::from_secs(30);

/// One count of the window's native view tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ViewCensus {
    /// `NSView`s under (and including) the content view.
    pub(crate) views: usize,
    /// `NSTrackingArea`s registered on those views, summed.
    pub(crate) tracking_areas: usize,
}

impl fmt::Display for ViewCensus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "views={} tracking_areas={}",
            self.views, self.tracking_areas
        )
    }
}

#[derive(Debug, Clone, Copy)]
struct Last {
    at_ms: u64,
    census: ViewCensus,
}

static LAST: Mutex<Option<Last>> = Mutex::new(None);

/// Whether a census is due: never taken, or [`CENSUS_EVERY`] since the last.
#[must_use]
pub(crate) fn due(last_at_ms: Option<u64>, now_ms: u64) -> bool {
    last_at_ms.is_none_or(|last| now_ms.saturating_sub(last) >= CENSUS_EVERY.as_millis() as u64)
}

/// When the last census was taken (wall-clock ms), if ever.
#[must_use]
pub(crate) fn last_at_ms() -> Option<u64> {
    LAST.lock()
        .ok()
        .and_then(|held| held.map(|last| last.at_ms))
}

/// Record a census: it becomes the one the hang report cites, and leaves a
/// `view_census` crumb so the ring shows the trend around a hang.
pub(crate) fn note(at_ms: u64, census: ViewCensus) {
    if let Ok(mut held) = LAST.lock() {
        *held = Some(Last { at_ms, census });
    }
    crate::crumbs::record("view_census", format_args!("{census}"));
}

/// The last census with its age, for a report line — `None` before the first.
#[must_use]
pub(crate) fn last_summary(now_ms: u64) -> Option<String> {
    let last = LAST.lock().ok().and_then(|held| *held)?;
    Some(format!(
        "{} (census age {}s)",
        last.census,
        now_ms.saturating_sub(last.at_ms) / 1000
    ))
}

/// Walk the main window's native view tree. Main thread only.
#[cfg(target_os = "macos")]
pub(crate) fn take(window: &tauri::WebviewWindow) -> Option<ViewCensus> {
    let raw = window.ns_window().ok()?;
    // SAFETY: `ns_window` hands back the live `NSWindow*` of this webview
    // window, and the walk runs on the main thread that owns AppKit objects.
    let ns_window: &objc2_app_kit::NSWindow = unsafe { &*raw.cast() };
    let content = ns_window.contentView()?;
    let mut census = ViewCensus {
        views: 0,
        tracking_areas: 0,
    };
    walk(&content, &mut census);
    Some(census)
}

#[cfg(target_os = "macos")]
fn walk(view: &objc2_app_kit::NSView, census: &mut ViewCensus) {
    census.views += 1;
    census.tracking_areas += view.trackingAreas().len();
    for sub in view.subviews().iter() {
        walk(&sub, census);
    }
}

/// No native view tree to count elsewhere.
#[cfg(not(target_os = "macos"))]
pub(crate) fn take(_window: &tauri::WebviewWindow) -> Option<ViewCensus> {
    None
}

#[cfg(test)]
mod tests {
    use super::{CENSUS_EVERY, ViewCensus, due};

    #[test]
    fn a_census_is_due_before_the_first_and_every_period_after() {
        let period = CENSUS_EVERY.as_millis() as u64;
        assert!(due(None, 0));
        assert!(!due(Some(1_000), 1_000 + period - 1));
        assert!(due(Some(1_000), 1_000 + period));
        // A clock that walked backwards does not underflow into "due".
        assert!(!due(Some(5_000), 4_000));
    }

    #[test]
    fn the_census_reads_as_two_counts() {
        let census = ViewCensus {
            views: 42,
            tracking_areas: 1_207,
        };
        assert_eq!(census.to_string(), "views=42 tracking_areas=1207");
    }
}
