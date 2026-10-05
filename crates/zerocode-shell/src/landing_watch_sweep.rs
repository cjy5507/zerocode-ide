//! The beat's look at late landings and runaway branches (t-34501 stage 2, t-22105).
//!
//! The ledger knows what a worker reported and what the coordinator wrote; git knows what is in main.
//! The sidebar already classifies every checkout against the compare ref and keeps the answer
//! (`worktree_landing`). This hands that answer — read from the cache, never from git — to the ledger
//! once a minute, and the ledger holds its record against it and writes what is late
//! (`Ledger::landing_watch`). Nothing here starts a process or a loop of its own: it rides the beat
//! that already exists, at a pace of its own, and a person can turn it off in the settings.

use super::*;

use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};

/// How often the beat looks. A late landing is hours old when it is told, so a minute is prompt, and
/// one ledger read per minute is nothing beside the one the board makes every second.
const WATCH_EVERY_MS: i64 = 60_000;

static ENABLED: AtomicBool = AtomicBool::new(true);
static LAST_LOOK_MS: AtomicI64 = AtomicI64::new(i64::MIN);

/// What a person set (the harness card's 「원장 알림」); on until they say otherwise.
pub(crate) fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::SeqCst);
}

pub(crate) fn sweep(app: &AppHandle, now_ms: i64) {
    if !ENABLED.load(Ordering::SeqCst) {
        return;
    }
    let last = LAST_LOOK_MS.load(Ordering::SeqCst);
    if now_ms.saturating_sub(last) < WATCH_EVERY_MS
        || LAST_LOOK_MS
            .compare_exchange(last, now_ms, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
    {
        return;
    }
    let occupied = crate::occupied_checkouts(&app.state::<AppState>());
    let witnesses = orchestration::watched_checkouts()
        .into_iter()
        .filter_map(|path| {
            let at = PathBuf::from(&path);
            let busy = crate::occupancy_of(&occupied, &at) > 0;
            crate::worktree_landing::witness_for(&at, busy)
        })
        .collect();
    orchestration::watch_landings(witnesses, now_ms);
}
