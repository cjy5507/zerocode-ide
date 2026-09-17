//! The native tray shortcut, close policy, and macOS waiting-activity mark.
//!
//! This module owns the native tray object, its menu ids, and the ordering
//! between live native changes and durable settings patches. The settings
//! document remains in `main`; native tray state does not leak into it.

use std::sync::{
    Mutex,
    atomic::{AtomicBool, Ordering},
};

use crate::ShellStateExt;
use tauri::AppHandle;

#[cfg(any(target_os = "macos", target_os = "windows"))]
use tauri::{Emitter, Manager, menu::MenuBuilder, tray::TrayIconBuilder};
#[cfg(target_os = "windows")]
use tauri_plugin_notification::NotificationExt;

#[cfg(any(target_os = "macos", target_os = "windows"))]
const TRAY_ID: &str = "zerocode-native-tray";
#[cfg(any(target_os = "macos", target_os = "windows"))]
const OPEN_ID: &str = "zerocode-native-tray-open";
#[cfg(any(target_os = "macos", target_os = "windows"))]
const SETTINGS_ID: &str = "zerocode-native-tray-settings";
#[cfg(any(target_os = "macos", target_os = "windows"))]
const QUIT_ID: &str = "zerocode-native-tray-quit";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ActivitySource {
    AgentTaskComplete,
    TerminalBell,
    AgentAttention,
}

#[must_use]
pub(crate) const fn should_mark_activity(source: ActivitySource, main_visible: bool) -> bool {
    !main_visible
        && matches!(
            source,
            ActivitySource::AgentTaskComplete | ActivitySource::TerminalBell
        )
}

#[must_use]
#[cfg(any(target_os = "windows", test))]
pub(crate) const fn should_hide_on_close(
    is_windows: bool,
    preference_enabled: bool,
    application_is_quitting: bool,
    tray_available: bool,
) -> bool {
    is_windows && preference_enabled && !application_is_quitting && tray_available
}

/// One process-wide owner for tray creation, removal, close policy, and marks.
pub(crate) struct NativeTray {
    mutation: Mutex<()>,
    minimize_on_close: AtomicBool,
    quitting: AtomicBool,
    #[cfg(target_os = "windows")]
    minimize_notice_shown: AtomicBool,
    #[cfg(target_os = "macos")]
    attention: AtomicBool,
}

impl Default for NativeTray {
    fn default() -> Self {
        Self {
            mutation: Mutex::new(()),
            minimize_on_close: AtomicBool::new(false),
            quitting: AtomicBool::new(false),
            #[cfg(target_os = "windows")]
            minimize_notice_shown: AtomicBool::new(false),
            #[cfg(target_os = "macos")]
            attention: AtomicBool::new(false),
        }
    }
}

impl NativeTray {
    /// Install the menu dispatcher once. Recreating a Tauri tray with a
    /// per-tray callback would retain another global callback each time.
    pub(crate) fn install_handler(&self, app: &AppHandle) {
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        app.on_menu_event(|app, event| match event.id().as_ref() {
            OPEN_ID => reveal_main(app, false),
            SETTINGS_ID => reveal_main(app, true),
            QUIT_ID => {
                let state = app.state::<crate::AppState>();
                crate::note_window_event(
                    state.local_data_root(),
                    "exit requested: native tray quit",
                );
                state.native_tray().begin_exit();
                crate::orchestration::window_exiting(crate::now_epoch_ms());
                app.exit(0);
            }
            _ => {}
        });

        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        let _ = app;
    }

    /// Restore both platform-specific preferences before the first close.
    pub(crate) fn sync_for_boot(
        &self,
        app: &AppHandle,
        show_menu_bar_icon: bool,
        minimize_on_close: bool,
    ) -> Result<(), String> {
        let _guard = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.minimize_on_close
            .store(minimize_on_close, Ordering::Relaxed);
        self.sync_unlocked(app, show_menu_bar_icon)
    }

    /// Serialize native preference changes with persistence and recover the
    /// native surface from the canonical document if the commit is refused.
    pub(crate) fn apply_menu_bar_preference<T>(
        &self,
        app: &AppHandle,
        visible: bool,
        persist: impl FnOnce() -> Result<T, String>,
        recover: impl FnOnce() -> bool,
    ) -> Result<T, String> {
        let _guard = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.sync_unlocked(app, visible)?;
        match persist() {
            Ok(value) => Ok(value),
            Err(error) => match self.sync_unlocked(app, recover()) {
                Ok(()) => Err(error),
                Err(rollback) => Err(format!(
                    "{error}; the menu-bar icon could not be restored: {rollback}"
                )),
            },
        }
    }

    /// Change the Windows close policy optimistically and restore the latest
    /// canonical value if the durable field patch is refused.
    pub(crate) fn apply_minimize_preference<T>(
        &self,
        enabled: bool,
        persist: impl FnOnce() -> Result<T, String>,
        recover: impl FnOnce() -> bool,
    ) -> Result<T, String> {
        let _guard = self
            .mutation
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.minimize_on_close.store(enabled, Ordering::Relaxed);
        match persist() {
            Ok(value) => Ok(value),
            Err(error) => {
                self.minimize_on_close.store(recover(), Ordering::Relaxed);
                Err(error)
            }
        }
    }

    /// Hide only a live main window that can be reopened from a live tray.
    #[must_use]
    pub(crate) fn hide_main_on_close(&self, app: &AppHandle) -> bool {
        #[cfg(target_os = "windows")]
        {
            if !should_hide_on_close(
                true,
                self.minimize_on_close.load(Ordering::Relaxed),
                self.quitting.load(Ordering::Relaxed),
                app.tray_by_id(TRAY_ID).is_some(),
            ) {
                return false;
            }
            let Some(window) = app.get_webview_window(crate::MAIN_WINDOW_LABEL) else {
                return false;
            };
            if window.hide().is_err() {
                return false;
            }
            self.notify_minimized_once(app);
            true
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = app;
            false
        }
    }

    pub(crate) fn begin_exit(&self) {
        self.quitting.store(true, Ordering::Relaxed);
    }

    /// Remember activity only while the icon exists and Orca would mark it.
    pub(crate) fn note_activity(&self, app: &AppHandle, source: ActivitySource) {
        #[cfg(target_os = "macos")]
        {
            let _guard = self
                .mutation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !should_mark_activity(source, main_window_visible(app))
                || app.tray_by_id(TRAY_ID).is_none()
            {
                return;
            }
            self.attention.store(true, Ordering::Relaxed);
            let _ = self.paint_attention(app);
        }

        #[cfg(not(target_os = "macos"))]
        let _ = (app, source);
    }

    pub(crate) fn clear_activity(&self, app: &AppHandle) {
        #[cfg(target_os = "macos")]
        {
            let _guard = self
                .mutation
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            self.attention.store(false, Ordering::Relaxed);
            let _ = self.paint_attention(app);
        }

        #[cfg(not(target_os = "macos"))]
        let _ = app;
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    fn sync_unlocked(&self, app: &AppHandle, visible: bool) -> Result<(), String> {
        if cfg!(target_os = "macos") && !visible {
            #[cfg(target_os = "macos")]
            self.attention.store(false, Ordering::Relaxed);
            drop(app.remove_tray_by_id(TRAY_ID));
            return Ok(());
        }

        if app.tray_by_id(TRAY_ID).is_none() {
            let product_name = app.package_info().name.as_str();
            let menu = MenuBuilder::new(app)
                .text(OPEN_ID, format!("Open {product_name}"))
                .text(SETTINGS_ID, "Settings…")
                .separator()
                .text(QUIT_ID, format!("Quit {product_name}"))
                .build()
                .map_err(|error| error.to_string())?;
            let icon = app
                .default_window_icon()
                .cloned()
                .ok_or_else(|| "the application has no menu-bar icon".to_string())?;
            TrayIconBuilder::with_id(TRAY_ID)
                .icon(icon)
                .tooltip(product_name)
                .menu(&menu)
                .show_menu_on_left_click(true)
                .build(app)
                .map_err(|error| error.to_string())?;
        }
        self.paint_attention(app)
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    fn sync_unlocked(&self, app: &AppHandle, visible: bool) -> Result<(), String> {
        let _ = (app, visible);
        Ok(())
    }

    #[cfg(target_os = "macos")]
    fn paint_attention(&self, app: &AppHandle) -> Result<(), String> {
        let Some(tray) = app.tray_by_id(TRAY_ID) else {
            return Ok(());
        };
        let active = self.attention.load(Ordering::Relaxed);
        tray.set_title(active.then_some("•"))
            .map_err(|error| error.to_string())?;
        let product_name = app.package_info().name.as_str();
        let tooltip = if active {
            format!("{product_name} — activity waiting")
        } else {
            product_name.to_string()
        };
        tray.set_tooltip(Some(tooltip))
            .map_err(|error| error.to_string())
    }

    #[cfg(target_os = "windows")]
    fn paint_attention(&self, app: &AppHandle) -> Result<(), String> {
        let _ = app;
        Ok(())
    }

    #[cfg(target_os = "windows")]
    fn notify_minimized_once(&self, app: &AppHandle) {
        if self.minimize_notice_shown.swap(true, Ordering::Relaxed) {
            return;
        }
        let product_name = app.package_info().name.as_str();
        let _ = app
            .notification()
            .builder()
            .title(product_name)
            .body(format!(
                "{product_name} is still running in the system tray"
            ))
            .show();
    }
}

#[cfg(target_os = "macos")]
fn main_window_visible(app: &AppHandle) -> bool {
    app.get_webview_window(crate::MAIN_WINDOW_LABEL)
        .and_then(|window| window.is_visible().ok())
        .unwrap_or(false)
}

#[cfg(any(target_os = "macos", target_os = "windows"))]
fn reveal_main(app: &AppHandle, open_settings: bool) {
    if let Some(window) = app.get_webview_window(crate::MAIN_WINDOW_LABEL) {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
    }
    app.state::<crate::AppState>()
        .native_tray()
        .clear_activity(app);
    if open_settings {
        let _ = app.emit_to(crate::MAIN_WINDOW_LABEL, "settings:open", ());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_hidden_completion_and_terminal_bells_mark_activity() {
        assert!(should_mark_activity(
            ActivitySource::AgentTaskComplete,
            false
        ));
        assert!(should_mark_activity(ActivitySource::TerminalBell, false));
        assert!(!should_mark_activity(ActivitySource::AgentAttention, false));
        assert!(!should_mark_activity(
            ActivitySource::AgentTaskComplete,
            true
        ));
        assert!(!should_mark_activity(ActivitySource::TerminalBell, true));

        assert!(should_hide_on_close(true, true, false, true));
        assert!(!should_hide_on_close(false, true, false, true));
        assert!(!should_hide_on_close(true, false, false, true));
        assert!(!should_hide_on_close(true, true, true, true));
        assert!(!should_hide_on_close(true, true, false, false));
    }
}
