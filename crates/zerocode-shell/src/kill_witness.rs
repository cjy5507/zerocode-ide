//! The window's side of the kill guard (t-80602): which process is the
//! calling pane's own, and which trees no other pane may signal — the window
//! itself and every pane. The rules and the process table are core's
//! ([`zerocode_core::shell_rule::kill`]); the bridge asks only for a command
//! that would signal something, on a blocking thread, and holds the answer
//! to its wall ([`zerocode_hookd::KILL_GUARD_WALL`]).

use std::sync::OnceLock;
use std::time::Duration;

use tauri::Manager as _;
use zerocode_core::shell_rule::kill::{ProcessTable, ProtectedRoot};
use zerocode_hookd::KillRoots;
use zerocode_lane::PtyTransport as _;

use crate::terminal_registry::lock_pty_within;
use crate::{AppState, ShellStateExt as _};

/// How long a pane's terminal is waited for while its pid is read. A pane
/// busy past it is left out: its processes still hang in the window's tree,
/// which is kept as a whole.
const PANE_PATIENCE: Duration = Duration::from_millis(50);

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
            protected: vec![ProtectedRoot::window(std::process::id())],
        };
        let Some(app) = WINDOW.get() else {
            return roots;
        };
        let state = app.state::<AppState>();
        for (term, held) in state.terminals().entries() {
            let Some(pid) = lock_pty_within(&held, PANE_PATIENCE).and_then(|pty| pty.pid()) else {
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
        ProcessTable::read(zerocode_hookd::KILL_GUARD_WALL)
    }
}
