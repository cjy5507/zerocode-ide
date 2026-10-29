//! The window's side of the kill guard (t-80602): which process is the
//! calling pane's own, and which trees no other pane may signal — the window
//! itself and every pane. The rules and the process table are core's
//! ([`zerocode_core::shell_rule::kill`]); the bridge asks only for a command
//! that would signal something, on a blocking thread, and holds the answer
//! to its wall ([`zerocode_hookd::KILL_GUARD_WALL`]).

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tauri::Manager as _;
use zerocode_core::shell_rule::kill::{ProcessTable, ProtectedRoot};
use zerocode_hookd::KillRoots;
use zerocode_lane::PtyTransport as _;

use crate::terminal_registry::lock_pty_within;
use crate::{AppState, ShellStateExt as _};

/// How long a pane's terminal is waited for while its pid is read: the
/// pump holds a pane for one drain round ([`crate::DRAIN_BUDGET`], 8 ms),
/// and the way out waits the same 50 ms for the same lock
/// (`system_runtime`'s `EXIT_LOCK_PATIENCE`). A pane busy past it is left
/// out: its processes still hang in the window's tree, which is kept as a
/// whole — and when it is the calling pane, its kill is refused, never let
/// through.
const PANE_PATIENCE: Duration = Duration::from_millis(50);

/// How long the panes are waited for in all, however many there are: four
/// panes held for their whole [`PANE_PATIENCE`]; the rest are then tried
/// once each, without waiting.
const ROOTS_WALL: Duration = Duration::from_millis(200);

/// What the process listing may take: the rest of the bridge's wall, so the
/// panes and the listing together stay inside it — 600 ms, thirty times the
/// 20 ms the listing took on this machine (five runs, 10-28).
const TABLE_WALL: Duration = zerocode_hookd::KILL_GUARD_WALL.saturating_sub(ROOTS_WALL);

/// How long one more pane is waited for when `spent` of [`ROOTS_WALL`] is
/// gone.
fn pane_patience(spent: Duration) -> Duration {
    ROOTS_WALL.saturating_sub(spent).min(PANE_PATIENCE)
}

static WINDOW: OnceLock<tauri::AppHandle> = OnceLock::new();

/// Bind the guard to this window's panes. Until then — and in a test that
/// never binds one — no pane is known: the window is still kept, and a
/// caller it cannot place owns nothing.
pub(crate) fn install_window(app: tauri::AppHandle) {
    let _ = WINDOW.set(app);
}

pub(crate) struct PaneProcesses;

impl zerocode_hookd::KillWitness for PaneProcesses {
    fn roots(&self, pane_key: &str) -> KillRoots {
        let caller_term = crate::hooks::term_of_pane_key(pane_key);
        let mut roots = KillRoots {
            caller: None,
            caller_mark: None,
            protected: vec![ProtectedRoot::window(std::process::id())],
        };
        let Some(app) = WINDOW.get() else {
            return roots;
        };
        let state = app.state::<AppState>();
        let started = Instant::now();
        for (term, held) in state.terminals().entries() {
            let patience = pane_patience(started.elapsed());
            let Some(pid) = lock_pty_within(&held, patience).and_then(|pty| pty.pid()) else {
                continue;
            };
            if caller_term == Some(term) {
                roots.caller = Some(pid);
            } else {
                roots.protected.push(ProtectedRoot {
                    pid,
                    label: format!("pane term-{term}"),
                });
            }
        }
        roots
    }

    fn table(&self) -> ProcessTable {
        ProcessTable::read(TABLE_WALL)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// However many panes are busy, their wait and the listing's stay inside
    /// the bridge's wall: a reply past it would be dropped, and a dropped
    /// refusal runs the command.
    #[test]
    fn the_panes_and_the_listing_share_the_bridges_wall() {
        assert_eq!(pane_patience(Duration::ZERO), PANE_PATIENCE);
        assert_eq!(
            pane_patience(ROOTS_WALL - Duration::from_millis(10)),
            Duration::from_millis(10)
        );
        assert_eq!(pane_patience(ROOTS_WALL), Duration::ZERO);
        assert_eq!(pane_patience(ROOTS_WALL * 3), Duration::ZERO);
        assert_eq!(ROOTS_WALL + TABLE_WALL, zerocode_hookd::KILL_GUARD_WALL);
    }
}
