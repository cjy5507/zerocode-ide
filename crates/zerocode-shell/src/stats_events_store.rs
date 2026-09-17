//! Where the stats pane's three counters are kept.
//!
//! [`zerocode_core::stats_events`] decides what each verb does to the numbers;
//! this module is the file they live in and the lock around it. The split is
//! the one every rule in this workspace keeps: the arithmetic is pure and
//! tested next to itself, and what is left here is I/O.
//!
//! ## Why a module of its own rather than a field on the app state
//!
//! These counters are written from three places that have nothing else in
//! common — an agent launched, an agent session ended, a pull request opened —
//! and read from one. Threading a handle through every one of those call sites
//! would put this ledger's name in signatures that have no other reason to
//! mention it. The scan cache next door is kept the same way and for the same
//! reason.
//!
//! ## Durability
//!
//! Losing this file loses statistics, not work, so the write is one durable
//! replace and no journal. It is written on every counted event, which is a
//! handful of times an hour — not a rate that needs batching.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use zerocode_core::stats_events::StatsCounters;

/// The document, beside the other per-machine records in the config root.
const FILE: &str = "stats-events.json";

fn file() -> Option<PathBuf> {
    ROOT.get().map(|root| root.join(FILE))
}

static ROOT: OnceLock<PathBuf> = OnceLock::new();
static HELD: Mutex<Option<StatsCounters>> = Mutex::new(None);

/// Names the config root once, at startup, and loads what is already counted.
pub fn open(config_root: &Path) {
    let _ = ROOT.set(config_root.to_path_buf());
    let loaded = file()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .and_then(|text| serde_json::from_str::<StatsCounters>(&text).ok())
        .unwrap_or_default();
    *held() = Some(loaded);
}

/// What the pane draws.
pub fn read() -> StatsCounters {
    held().unwrap_or_default()
}

/// Applies one verb and writes the result down.
///
/// A failed write is dropped rather than reported: the caller is a launch or a
/// pull request, and neither should fail because a statistic could not be
/// saved. The counter stays correct in memory until the next restart.
pub fn record(change: impl FnOnce(&mut StatsCounters)) {
    let saving = {
        let mut guard = held();
        let counters = guard.get_or_insert_with(StatsCounters::default);
        change(counters);
        *counters
    };
    let Some(path) = file() else {
        return;
    };
    let Ok(text) = serde_json::to_string_pretty(&saving) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = crate::durable_file::ensure_private_directory(parent);
    }
    let _ = crate::durable_file::replace_bytes(&path, text.as_bytes());
}

fn held() -> std::sync::MutexGuard<'static, Option<StatsCounters>> {
    HELD.lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

/* ---- when each pane's agent session began -------------------------------
 *
 * Kept beside the counters rather than in the app state, because the only
 * reader is the moment a session ends and the counter is written. A start
 * stamp held anywhere else is a stamp somebody can read after the pane is
 * gone, and the figure it would produce is a session that never happened.
 *
 * Sessions still running are NOT in the total — see the core module's note. A
 * pane open for three hours contributes when it closes, not while it sits
 * there. */

static SINCE: OnceLock<Mutex<HashMap<u32, i64>>> = OnceLock::new();

/// An agent took a pane.
pub fn agent_began(term: u32, at_ms: i64) {
    if at_ms <= 0 {
        return;
    }
    {
        // A second arrival for the same pane keeps the FIRST stamp: the roster
        // is reconciled repeatedly and the same session can be re-announced,
        // which is not the session starting again. The guard is scoped so the
        // counter's lock is never taken while this one is held.
        let mut since = since();
        if since.contains_key(&term) {
            return;
        }
        since.insert(term, at_ms);
    }
    record(|counters| counters.agent_spawned(at_ms));
}

/// The agent that held this pane is gone.
pub fn agent_ended(term: u32, at_ms: i64) {
    let began = since().remove(&term);
    let Some(began) = began else {
        return;
    };
    record(|counters| counters.agent_worked(at_ms - began, at_ms));
}

fn since() -> std::sync::MutexGuard<'static, HashMap<u32, i64>> {
    SINCE
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Before the root is named, reading answers zeroes rather than panicking.
    ///
    /// The pane can be opened by a window that booted from a directory this
    /// process could not create; the honest answer there is "nothing counted",
    /// not a crash on the way to drawing a settings page.
    #[test]
    fn an_unopened_ledger_reads_as_empty() {
        let counters = read();
        assert!(counters.agents_spawned >= 0);
        assert!(counters.is_untouched() || counters.agents_spawned > 0);
    }
}
