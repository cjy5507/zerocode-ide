//! Where each pane's foreground process is standing.
//!
//! A tab is born in a worktree and keeps that name, but the agent inside it
//! can walk away: zo's `EnterWorktree` moves the process into a checkout of
//! its own, and every `cd` moves a shell. The sidebar reads the tab, so the
//! work then shows under the wrong card and the real worktree looks empty
//! (live report 2026-09-09, 「워커 네비바와 워크트리 불일치」). This sweep asks
//! the kernel where the foreground process of every held pane stands and
//! tells the window when the answer moves; the window moves the tab.
use super::*;

/// How often the pool is asked. One `lsof` for every pane per look — a
/// person's pace, not the pump's.
pub(super) const CWD_LOOK_EVERY: Duration = Duration::from_secs(5);

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct TermCwd {
    pub(super) term: TermId,
    pub(super) cwd: String,
}

/// The panes whose answer differs from what the window was last told — a
/// first answer counts, because the window knows nothing yet. Ordered by
/// pane so two sweeps over the same pool emit the same sequence.
pub(super) fn changes(
    known: &HashMap<TermId, String>,
    seen: &HashMap<TermId, String>,
) -> Vec<TermCwd> {
    let mut moved: Vec<TermCwd> = seen
        .iter()
        .filter(|(term, cwd)| known.get(*term) != Some(*cwd))
        .map(|(term, cwd)| TermCwd {
            term: *term,
            cwd: cwd.clone(),
        })
        .collect();
    moved.sort_by_key(|change| change.term);
    moved
}

/// Ask, but not on the beat that has to draw.
///
/// [`sweep_pane_cwds`] forks `lsof` and waits for its answer. Measured on this
/// machine over six pids: **77–84 ms**, five times in a row. On the pump
/// thread that is a render loop stopped dead for five frames every
/// [`CWD_LOOK_EVERY`] — a hitch on a timer, which is the shape of stutter a
/// person reports as *regular* rather than random, and the shape no amount of
/// making the frame path faster can fix.
///
/// The pump keeps the CADENCE — one clock owner, as the loop's documentation
/// has it — and gives up only the waiting. Nothing downstream changes: the
/// sweep still emits `term:cwd` itself, from whichever thread it finished on,
/// because `emit` is the same call either way.
///
/// One sweep at a time. `lsof` under load can outrun this interval, and a
/// timer that spawned regardless would pile threads behind it — each holding
/// pane locks — until the slow thing it is waiting for got slower. Falling
/// behind honestly is the better failure: a skipped look costs a pane's card
/// five more seconds of naming the wrong worktree.
pub(super) fn sweep_pane_cwds_off_the_beat(app: &AppHandle) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SWEEPING: AtomicBool = AtomicBool::new(false);

    /// Clears the flag even if the sweep panics — a one-way latch here would
    /// stop every later look with nothing saying why.
    struct Sweeping;
    impl Drop for Sweeping {
        fn drop(&mut self) {
            SWEEPING.store(false, Ordering::Release);
        }
    }

    if SWEEPING.swap(true, Ordering::AcqRel) {
        return;
    }
    let app = app.clone();
    if std::thread::Builder::new()
        .name("pane-cwd-sweep".into())
        .spawn(move || {
            let _clear = Sweeping;
            sweep_pane_cwds(&app);
        })
        .is_err()
    {
        // The thread never started, so nothing will clear the flag for us.
        SWEEPING.store(false, Ordering::Release);
    }
}

/// Ask every held pane where its foreground process stands, then tell the
/// window about the panes that moved. Every pane, not only those with
/// something in front of the window's own shell: a resumed agent is spawned
/// as the pty's child itself (zo 43473 under the window, 2026-09-09), so
/// "the child is in front" says nothing about whether an agent is there.
/// Which panes may follow their process is the window's policy — it knows
/// which tabs are agents.
pub(super) fn sweep_pane_cwds(app: &AppHandle) {
    // Taken under each pane's own terminal lock with nothing else done under
    // it, the same discipline as the agent roll call beside this.
    let fronts: Vec<(TermId, u32)> = {
        let state = app.state::<AppState>();
        let entries = state.terminals().entries();
        entries
            .iter()
            .filter_map(|(term, held)| {
                lock_pty(held)
                    .foreground_process_id()
                    .map(|pid| (*term, pid))
            })
            .collect()
    };
    if fronts.is_empty() {
        let state = app.state::<AppState>();
        state.pane_cwds().clear();
        return;
    }
    let pids: Vec<u32> = fronts.iter().map(|(_, pid)| *pid).collect();
    let cwds = system_runtime::process_cwds(&pids);
    let seen: HashMap<TermId, String> = fronts
        .iter()
        .filter_map(|(term, pid)| cwds.get(pid).map(|cwd| (*term, cwd.clone())))
        .collect();
    let moved = {
        let state = app.state::<AppState>();
        let mut known = state.pane_cwds();
        let moved = changes(&known, &seen);
        *known = seen;
        moved
    };
    for change in moved {
        let _ = app.emit("term:cwd", change);
    }
}
