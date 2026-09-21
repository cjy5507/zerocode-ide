//! The three preferences the emulator lifetimes are decided by.
//!
//! One reader for both platform halves. The keys themselves live in the
//! settings document's `emulator` table, written by the window's settings
//! surface; everything here only READS them, and reads them fresh — a person
//! who turns the idle sweep off should not have to restart the window for the
//! sweep to hear about it.
//!
//! Read out of the document as JSON rather than out of its typed fields, on
//! purpose: this half of the design (the iOS lifetimes) lands beside the half
//! that adds the table, and a reader that falls back to the documented
//! defaults works before, during and after that merge instead of failing to
//! compile until both halves are in. Both spellings of a key are accepted for
//! the same reason — serde renames nested tables to camelCase in some of this
//! document and leaves them snake_case in others, and which one this table
//! ends up wearing is not a fact this reader should have an opinion about.

use std::time::Duration;

use tauri::Manager;

/// The settings table all three keys live in.
const EMULATOR_TABLE: &str = "emulator";

/// Whether devices this window booted are left running when it closes.
const KEEP_BOOTED_KEY: &str = "keepBooted";
/// Kept, by default: a device that survives a window restart is the whole
/// point of the first second this design is about (D3).
const KEEP_BOOTED_DEFAULT: bool = true;

/// Whether the last device used is woken at window boot.
const PREBOOT_LAST_USED_KEY: &str = "prebootLastUsed";
const PREBOOT_LAST_USED_DEFAULT: bool = true;

/// How many minutes a device may sit with no pane before it is shut down.
const IDLE_SHUTDOWN_MINUTES_KEY: &str = "idleShutdownMinutes";
const IDLE_SHUTDOWN_MINUTES_DEFAULT: u64 = 30;
/// The number that turns the sweep off entirely.
const IDLE_SHUTDOWN_NEVER: u64 = 0;
const SECONDS_PER_MINUTE: u64 = 60;

/// The emulator lifetime preferences, as any road here reads them.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct EmulatorPrefs {
    /// Leave the devices this window booted running when it closes.
    pub(super) keep_booted: bool,
    /// Wake the last device used when the window starts.
    pub(super) preboot_last_used: bool,
    /// How long a device may go without a pane before it is reclaimed, or
    /// `None` when the person has turned that off.
    pub(super) idle_shutdown: Option<Duration>,
}

impl Default for EmulatorPrefs {
    fn default() -> Self {
        Self {
            keep_booted: KEEP_BOOTED_DEFAULT,
            preboot_last_used: PREBOOT_LAST_USED_DEFAULT,
            idle_shutdown: minutes_to_wait(IDLE_SHUTDOWN_MINUTES_DEFAULT),
        }
    }
}

/// A key's value under either spelling, or nothing.
fn under<'a>(table: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    table
        .get(key)
        .or_else(|| table.get(snake_spelling(key)))
        .filter(|value| !value.is_null())
}

/// `keepBooted` as `keep_booted`. One place, so a key is named once.
fn snake_spelling(key: &str) -> String {
    let mut spelled = String::with_capacity(key.len() + 2);
    for letter in key.chars() {
        if letter.is_ascii_uppercase() {
            spelled.push('_');
            spelled.push(letter.to_ascii_lowercase());
        } else {
            spelled.push(letter);
        }
    }
    spelled
}

/// The wait a minute count stands for — and `None` for the count that means
/// "never", which is the only way to say off in one number.
fn minutes_to_wait(minutes: u64) -> Option<Duration> {
    (minutes != IDLE_SHUTDOWN_NEVER)
        .then(|| Duration::from_secs(minutes.saturating_mul(SECONDS_PER_MINUTE)))
}

impl EmulatorPrefs {
    /// Read the three keys out of a settings document, defaulting each one on
    /// its own: a table that carries two of them is not a reason to guess at
    /// the third.
    pub(super) fn read(document: &serde_json::Value) -> Self {
        let fallback = Self::default();
        let Some(table) = under(document, EMULATOR_TABLE) else {
            return fallback;
        };
        Self {
            keep_booted: under(table, KEEP_BOOTED_KEY)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(fallback.keep_booted),
            preboot_last_used: under(table, PREBOOT_LAST_USED_KEY)
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(fallback.preboot_last_used),
            idle_shutdown: under(table, IDLE_SHUTDOWN_MINUTES_KEY)
                .and_then(serde_json::Value::as_u64)
                .map_or(fallback.idle_shutdown, minutes_to_wait),
        }
    }
}

/// The preferences as they stand right now.
///
/// A document that cannot be read at all is not an excuse to shut somebody's
/// devices down or to leave them running against their wishes — it is the
/// documented defaults, which is what a window with no settings file has
/// always used.
pub(super) fn of(app: &tauri::AppHandle) -> EmulatorPrefs {
    let document =
        crate::settings_runtime::load_settings_resilient(app.state::<crate::AppState>().settings())
            .document;
    EmulatorPrefs::read(&serde_json::to_value(&document).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document(table: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "locale": "ko", EMULATOR_TABLE: table })
    }

    /// A document written before the table exists reads as the design's
    /// defaults, which is what makes this half shippable on its own.
    #[test]
    fn a_document_without_the_table_reads_as_the_defaults() {
        let read = EmulatorPrefs::read(&serde_json::json!({ "locale": "ko" }));
        assert_eq!(read, EmulatorPrefs::default());
        assert!(read.keep_booted && read.preboot_last_used);
        assert_eq!(
            read.idle_shutdown,
            Some(Duration::from_secs(IDLE_SHUTDOWN_MINUTES_DEFAULT * 60))
        );
        // A table that is there but empty says nothing either, and a null
        // value is a key nobody has answered rather than a `false`.
        assert_eq!(
            EmulatorPrefs::read(&document(serde_json::json!({}))),
            EmulatorPrefs::default()
        );
        assert_eq!(
            EmulatorPrefs::read(&document(serde_json::json!({ "keepBooted": null }))),
            EmulatorPrefs::default()
        );
    }

    /// Either spelling answers, so this reader does not have to know which one
    /// the table that holds the keys ended up wearing.
    #[test]
    fn both_spellings_of_a_key_are_the_same_key() {
        for table in [
            serde_json::json!({ "keepBooted": false, "prebootLastUsed": false, "idleShutdownMinutes": 5 }),
            serde_json::json!({ "keep_booted": false, "preboot_last_used": false, "idle_shutdown_minutes": 5 }),
        ] {
            let read = EmulatorPrefs::read(&document(table));
            assert!(!read.keep_booted);
            assert!(!read.preboot_last_used);
            assert_eq!(read.idle_shutdown, Some(Duration::from_secs(5 * 60)));
        }
        assert_eq!(snake_spelling(KEEP_BOOTED_KEY), "keep_booted");
        assert_eq!(
            snake_spelling(IDLE_SHUTDOWN_MINUTES_KEY),
            "idle_shutdown_minutes"
        );
    }

    /// Zero minutes is how "never reclaim" is said in one number, and it must
    /// not read as "reclaim immediately".
    #[test]
    fn zero_minutes_turns_the_sweep_off_rather_than_making_it_instant() {
        let read = EmulatorPrefs::read(&document(
            serde_json::json!({ IDLE_SHUTDOWN_MINUTES_KEY: IDLE_SHUTDOWN_NEVER }),
        ));
        assert_eq!(read.idle_shutdown, None);
        // And every other key is still the one the person set.
        assert!(read.keep_booted);
    }

    /// A value of the wrong shape is a document this reader does not get to
    /// act on — the default stands.
    #[test]
    fn a_value_of_the_wrong_shape_falls_back_rather_than_being_coerced() {
        let read = EmulatorPrefs::read(&document(serde_json::json!({
            KEEP_BOOTED_KEY: "yes",
            IDLE_SHUTDOWN_MINUTES_KEY: -5,
        })));
        assert_eq!(read, EmulatorPrefs::default());
    }
}
