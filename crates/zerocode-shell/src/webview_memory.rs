use tauri::AppHandle;

#[cfg(any(target_os = "windows", test))]
use std::collections::BTreeMap;
#[cfg(target_os = "windows")]
use std::sync::{Mutex, OnceLock, PoisonError};
#[cfg(target_os = "windows")]
use std::time::Duration;

#[cfg(target_os = "windows")]
const IDLE_LOW_GRACE: Duration = Duration::from_secs(10);

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FocusAction {
    Noop,
    ApplyNormal,
    ScheduleLow { generation: u64 },
}

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Default)]
struct WindowFocusState {
    focused: bool,
    generation: u64,
    is_low: bool,
}

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Default)]
struct MemoryTargetGate {
    windows: BTreeMap<String, WindowFocusState>,
}

#[cfg(any(target_os = "windows", test))]
impl MemoryTargetGate {
    fn note_focus(&mut self, label: &str, focused: bool) -> FocusAction {
        let entry = self.windows.entry(label.to_string()).or_default();
        entry.generation = entry.generation.wrapping_add(1);
        entry.focused = focused;
        if focused {
            if entry.is_low {
                entry.is_low = false;
                FocusAction::ApplyNormal
            } else {
                FocusAction::Noop
            }
        } else if entry.is_low {
            FocusAction::Noop
        } else {
            FocusAction::ScheduleLow {
                generation: entry.generation,
            }
        }
    }

    fn take_due_low(&mut self, label: &str, generation: u64) -> bool {
        let Some(entry) = self.windows.get_mut(label) else {
            return false;
        };
        if entry.focused || entry.generation != generation || entry.is_low {
            return false;
        }
        entry.is_low = true;
        true
    }

    fn forget(&mut self, label: &str) {
        self.windows.remove(label);
    }
}

#[cfg(target_os = "windows")]
fn gate() -> &'static Mutex<MemoryTargetGate> {
    static GATE: OnceLock<Mutex<MemoryTargetGate>> = OnceLock::new();
    GATE.get_or_init(|| Mutex::new(MemoryTargetGate::default()))
}

#[cfg(target_os = "windows")]
fn apply_target(app: &AppHandle, label: &str, low: bool) {
    use tauri::Manager as _;
    use webview2_com::Microsoft::Web::WebView2::Win32::{
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW, COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL,
        ICoreWebView2_19,
    };
    use windows::core::Interface as _;

    let Some(window) = app.get_webview_window(label) else {
        return;
    };
    let target = if low {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_LOW
    } else {
        COREWEBVIEW2_MEMORY_USAGE_TARGET_LEVEL_NORMAL
    };
    let _ = window.with_webview(move |webview| unsafe {
        let _ = webview
            .controller()
            .CoreWebView2()
            .and_then(|core| core.cast::<ICoreWebView2_19>())
            .and_then(|memory| memory.SetMemoryUsageTargetLevel(target));
    });
}

#[cfg(target_os = "windows")]
pub(crate) fn note_window_focus(app: &AppHandle, label: &str, focused: bool) {
    let action = gate()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .note_focus(label, focused);
    match action {
        FocusAction::Noop => {}
        FocusAction::ApplyNormal => apply_target(app, label, false),
        FocusAction::ScheduleLow { generation } => {
            let app = app.clone();
            let label = label.to_string();
            let _ = std::thread::Builder::new()
                .name("webview-memory-low".into())
                .spawn(move || {
                    std::thread::sleep(IDLE_LOW_GRACE);
                    let due = gate()
                        .lock()
                        .unwrap_or_else(PoisonError::into_inner)
                        .take_due_low(&label, generation);
                    if due {
                        apply_target(&app, &label, true);
                    }
                });
        }
    }
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn note_window_focus(_app: &AppHandle, _label: &str, _focused: bool) {}

#[cfg(target_os = "windows")]
pub(crate) fn forget_window(label: &str) {
    gate()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .forget(label);
}

#[cfg(not(target_os = "windows"))]
pub(crate) fn forget_window(_label: &str) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quick_alt_tab_cancels_scheduled_low_target() {
        let mut gate = MemoryTargetGate::default();
        assert_eq!(gate.note_focus("main", true), FocusAction::Noop);
        let FocusAction::ScheduleLow { generation } = gate.note_focus("main", false) else {
            panic!("expected ScheduleLow on blur");
        };
        assert_eq!(gate.note_focus("main", true), FocusAction::Noop);
        assert!(!gate.take_due_low("main", generation));
    }

    #[test]
    fn sustained_unfocus_applies_low_and_refocus_restores_normal() {
        let mut gate = MemoryTargetGate::default();
        let FocusAction::ScheduleLow { generation } = gate.note_focus("main", false) else {
            panic!("expected ScheduleLow on blur");
        };
        assert!(gate.take_due_low("main", generation));
        assert!(!gate.take_due_low("main", generation));
        assert_eq!(gate.note_focus("main", false), FocusAction::Noop);
        assert_eq!(gate.note_focus("main", true), FocusAction::ApplyNormal);
        assert_eq!(gate.note_focus("main", true), FocusAction::Noop);
    }

    #[test]
    fn forgotten_window_ignores_late_low_timer() {
        let mut gate = MemoryTargetGate::default();
        let FocusAction::ScheduleLow { generation } = gate.note_focus("popout", false) else {
            panic!("expected ScheduleLow on blur");
        };
        gate.forget("popout");
        assert!(!gate.take_due_low("popout", generation));
    }
}
