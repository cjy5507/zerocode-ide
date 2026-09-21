//! The three preferences the emulator lifetimes are decided by.
//!
//! One reader for both platform halves, over the settings document the
//! window already holds typed (`settings_runtime::SettingsDocument`, keys
//! `emulator.keepBooted` · `emulator.prebootLastUsed` ·
//! `emulator.idleShutdownMinutes`, defaults beside every other setting).
//! Everything here only READS them, and reads them fresh — a person who turns
//! the idle sweep off should not have to restart the window for the sweep to
//! hear about it. A second parse of the document as JSON was the first shape
//! of this file; it looked for a nested `emulator` table the document never
//! writes (its keys are flat, dotted), so it would have answered the defaults
//! whatever the person chose.

use std::time::Duration;

use tauri::Manager;

/// Zero minutes: an idle fleet is never put away.
const IDLE_SHUTDOWN_NEVER: u32 = 0;
const SECONDS_PER_MINUTE: u64 = 60;

/// The person's three emulator switches, read off the settings document the
/// window already holds typed (`settings_runtime::SettingsDocument`) — the
/// defaults live there, once, beside every other setting; this is the one
/// reader the emulator module has, so the boot door, the exit and the idle
/// reclaimers cannot disagree about what the person asked for.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct EmulatorPrefs {
    pub(super) keep_booted: bool,
    pub(super) preboot_last_used: bool,
    /// `None` is "never put an idle fleet away" (`0` minutes).
    pub(super) idle_shutdown: Option<Duration>,
}

fn minutes_to_wait(minutes: u32) -> Option<Duration> {
    (minutes != IDLE_SHUTDOWN_NEVER)
        .then(|| Duration::from_secs(u64::from(minutes).saturating_mul(SECONDS_PER_MINUTE)))
}

impl EmulatorPrefs {
    pub(super) fn read(document: &crate::settings_runtime::SettingsDocument) -> Self {
        Self {
            keep_booted: document.emulator_keep_booted,
            preboot_last_used: document.emulator_preboot_last_used,
            idle_shutdown: minutes_to_wait(document.emulator_idle_shutdown_minutes),
        }
    }
}

pub(super) fn of(app: &tauri::AppHandle) -> EmulatorPrefs {
    let document =
        crate::settings_runtime::load_settings_resilient(app.state::<crate::AppState>().settings())
            .document;
    EmulatorPrefs::read(&document)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings_runtime::SettingsDocument;

    /// The reader is the typed document's own answer: a person who never
    /// chose keeps the fleet, preboots the last device and puts an idle
    /// fleet away after the documented number of minutes.
    #[test]
    fn a_person_who_never_chose_gets_the_documents_defaults() {
        let prefs = EmulatorPrefs::read(&SettingsDocument::default());
        assert!(prefs.keep_booted);
        assert!(prefs.preboot_last_used);
        assert_eq!(
            prefs.idle_shutdown,
            Some(Duration::from_secs(
                u64::from(SettingsDocument::default().emulator_idle_shutdown_minutes)
                    * SECONDS_PER_MINUTE
            ))
        );
    }

    /// Zero minutes is "never", and the two switches are read as written.
    #[test]
    fn zero_minutes_is_never_and_the_switches_are_read_as_written() {
        let document = SettingsDocument {
            emulator_keep_booted: false,
            emulator_preboot_last_used: false,
            emulator_idle_shutdown_minutes: IDLE_SHUTDOWN_NEVER,
            ..SettingsDocument::default()
        };
        let prefs = EmulatorPrefs::read(&document);
        assert!(!prefs.keep_booted);
        assert!(!prefs.preboot_last_used);
        assert_eq!(prefs.idle_shutdown, None);
        let later = SettingsDocument {
            emulator_idle_shutdown_minutes: 45,
            ..document
        };
        assert_eq!(
            EmulatorPrefs::read(&later).idle_shutdown,
            Some(Duration::from_secs(45 * SECONDS_PER_MINUTE))
        );
    }
}
