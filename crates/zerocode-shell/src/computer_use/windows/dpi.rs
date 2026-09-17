//! One coordinate contract for the provider: physical pixels everywhere.
//!
//! `GetWindowRect`, `WindowFromPoint`, `PrintWindow`/`BitBlt`, the virtual
//! screen metrics and `SendInput` all answer in the coordinate space of the
//! CALLING THREAD's DPI awareness, while UI Automation's bounding rectangles
//! are always physical. The window process is per-monitor-v2 aware because
//! tao made it so at startup — but that is a fact about the main thread's
//! process default, and a test process under cargo has no such default. So
//! the provider thread and the fixture threads set the context themselves,
//! explicitly, and the tests check it rather than assume it. Per-thread, so
//! nothing here touches tao's thread or any other.

use windows::Win32::UI::HiDpi::{
    AreDpiAwarenessContextsEqual, DPI_AWARENESS_CONTEXT,
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetThreadDpiAwarenessContext,
    SetThreadDpiAwarenessContext,
};

/// The calling thread's DPI awareness, set to per-monitor-v2 for as long as
/// this value lives and put back to what it was when it is dropped.
pub(super) struct ThreadDpiAwareness {
    previous: DPI_AWARENESS_CONTEXT,
}

impl ThreadDpiAwareness {
    /// Make this thread speak physical pixels. `previous` is what the
    /// thread had; the provider thread keeps the guard for its whole life,
    /// a test thread drops it at the end of the test.
    pub fn per_monitor_v2() -> Self {
        // SAFETY: a plain per-thread setting; the previous value is kept
        // for Drop.
        let previous =
            unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
        Self { previous }
    }

    /// Whether the calling thread is per-monitor-v2 aware right now.
    #[must_use]
    pub fn is_per_monitor_v2() -> bool {
        // SAFETY: plain reads.
        unsafe {
            AreDpiAwarenessContextsEqual(
                GetThreadDpiAwarenessContext(),
                DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
            )
        }
        .as_bool()
    }

    /// The contract, for reports: what this thread's coordinates mean.
    #[must_use]
    pub fn describe() -> &'static str {
        if Self::is_per_monitor_v2() {
            "per-monitor-v2 (physical pixels)"
        } else {
            "not per-monitor-v2 (virtualized coordinates; the provider would disagree with UI Automation)"
        }
    }
}

impl Drop for ThreadDpiAwareness {
    fn drop(&mut self) {
        // SAFETY: restores the value `per_monitor_v2` read on this thread.
        // A null previous context (the thread had none) is left as it is.
        if !self.previous.0.is_null() {
            unsafe { SetThreadDpiAwarenessContext(self.previous) };
        }
    }
}
