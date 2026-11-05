//! What a released worker's pane leaves running, ended by the pids the window
//! itself started for it (t-94842).
//!
//! `worker-release` closes the pane: the window hangs up its own child and
//! kills it a fifth of a second later (`zerocode_pty`). What that child started
//! and put out of the pane's way — a tool's command in a process group of its
//! own, a server it detached, anything that ignores the hang-up — is not the
//! child, and outlives it under launchd: the memory a person sees the window
//! hold after a worker finished.
//!
//! So the tree is read while the child still stands: the child and every
//! process under it by parent, each named by its pid AND its start identity.
//! On the beats after the close, a process of that tree still standing with
//! the same identity is ended by that pid — a polite signal first, the hard
//! one after. Nothing is found by its name, no pattern is matched, and nothing
//! outside that tree is ever signalled: a person's terminal is not a released
//! worker's pane, a pane a person took over is never released by the sweep, and
//! a pid some other program took since the close carries another start
//! identity and is left alone.

use std::sync::{Mutex, OnceLock};

/// How long the pane's own close has to end its tree before anything is
/// signalled: the hang-up reaches the child's group at once, and a CLI that
/// heard it is gone in well under a second.
pub(crate) const LINGER_MS: i64 = 3_000;
/// How long a polite signal has before the hard one, and the hard one before
/// the window stops watching and says what stood.
pub(crate) const SIGNAL_WAIT_MS: i64 = 3_000;

/// One process of a pane's tree, named the way a later look can tell it from
/// a program that took its pid since.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Named {
    pub(crate) pid: u32,
    pub(crate) started: String,
}

/// One row of a process table read: pid, parent pid, start identity.
pub(crate) type Row = (u32, u32, String);

/// The child at `root` and every process under it by parent, in `rows`. Empty
/// when the root is not in the table — a child already gone has no tree to
/// read, and its pid names nothing of the pane's.
pub(crate) fn tree_under(rows: &[Row], root: u32) -> Vec<Named> {
    let Some(first) = rows.iter().find(|(pid, _, _)| *pid == root) else {
        return Vec::new();
    };
    let mut tree = vec![Named {
        pid: first.0,
        started: first.2.clone(),
    }];
    let mut next = 0;
    while next < tree.len() {
        let parent = tree[next].pid;
        next += 1;
        for (pid, ppid, started) in rows {
            // A pid is in the tree once: a table whose rows loop on each other
            // must not grow it forever.
            if *ppid == parent && *pid != parent && !tree.iter().any(|one| one.pid == *pid) {
                tree.push(Named {
                    pid: *pid,
                    started: started.clone(),
                });
            }
        }
    }
    tree
}

/// The processes of `tree` that still stand as themselves in `rows`: the same
/// pid with the same start identity. A pid some other program took since is
/// not one of them, and an identity nobody could read matches nothing.
pub(crate) fn standing(tree: &[Named], rows: &[Row]) -> Vec<u32> {
    tree.iter()
        .filter(|one| {
            !one.started.is_empty()
                && rows
                    .iter()
                    .any(|(pid, _, started)| *pid == one.pid && *started == one.started)
        })
        .map(|one| one.pid)
        .collect()
}

/// A released pane's tree, watched until it is gone. `owner` names whose
/// pane it was, for the window log: a released worker (「worker w-1」), or
/// a tab the window put to sleep (low-spec 6, 「sleeping tab 5」).
#[derive(Debug, Clone)]
pub(crate) struct Watch {
    pub(crate) owner: String,
    pub(crate) term: u32,
    pub(crate) tree: Vec<Named>,
    pub(crate) closed_ms: i64,
    polite_ms: Option<i64>,
    hard_ms: Option<i64>,
}

impl Watch {
    pub(crate) fn new(owner: &str, term: u32, tree: Vec<Named>, closed_ms: i64) -> Self {
        Self {
            owner: owner.to_string(),
            term,
            tree,
            closed_ms,
            polite_ms: None,
            hard_ms: None,
        }
    }
}

/// What one beat does about one watch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Step {
    /// Nothing of the tree stands: the pane's close ended it all.
    Gone,
    /// Something stands, and its time is not up yet.
    Wait,
    /// End these pids: politely (`false`) or hard (`true`).
    Signal(Vec<u32>, bool),
    /// These still stood after the hard signal; the window stops watching.
    GaveUp(Vec<u32>),
}

/// What the watch calls for at `now_ms`, given which of its pids stand.
pub(crate) fn step(watch: &Watch, standing: &[u32], now_ms: i64) -> Step {
    if standing.is_empty() {
        return Step::Gone;
    }
    let since = |at: i64| now_ms.saturating_sub(at);
    match (watch.polite_ms, watch.hard_ms) {
        _ if since(watch.closed_ms) < LINGER_MS => Step::Wait,
        (None, _) => Step::Signal(standing.to_vec(), false),
        (Some(polite), None) if since(polite) < SIGNAL_WAIT_MS => Step::Wait,
        (Some(_), None) => Step::Signal(standing.to_vec(), true),
        (_, Some(hard)) if since(hard) < SIGNAL_WAIT_MS => Step::Wait,
        (_, Some(_)) => Step::GaveUp(standing.to_vec()),
    }
}

/// Every released pane being watched, and what each beat did about them.
#[derive(Debug, Default)]
pub(crate) struct Reaper {
    watches: Vec<Watch>,
}

impl Reaper {
    pub(crate) fn watch(&mut self, watch: Watch) {
        if watch.tree.is_empty() {
            return;
        }
        self.watches.retain(|held| held.term != watch.term);
        self.watches.push(watch);
    }

    pub(crate) fn is_idle(&self) -> bool {
        self.watches.is_empty()
    }

    /// One beat over a fresh table: every watch read against it, every signal
    /// sent by pid through `signal` (`true` = hard), and a line for each pane
    /// whose tree is done with — gone, ended, or given up on.
    pub(crate) fn beat(
        &mut self,
        rows: &[Row],
        now_ms: i64,
        signal: &mut dyn FnMut(u32, bool) -> bool,
    ) -> Vec<String> {
        let mut lines = Vec::new();
        self.watches.retain_mut(|watch| {
            let left = standing(&watch.tree, rows);
            match step(watch, &left, now_ms) {
                Step::Gone => {
                    let signalled = watch.polite_ms.is_some();
                    lines.push(format!(
                        "{}'s released pane (terminal {}): its {} process(es) {}",
                        watch.owner,
                        watch.term,
                        watch.tree.len(),
                        if signalled {
                            "ended after the window signalled the ones the close left"
                        } else {
                            "ended with the close"
                        }
                    ));
                    false
                }
                Step::Wait => true,
                Step::Signal(pids, hard) => {
                    let sent: Vec<u32> = pids
                        .iter()
                        .copied()
                        .filter(|pid| signal(*pid, hard))
                        .collect();
                    lines.push(format!(
                        "{}'s released pane (terminal {}): {} process(es) of its own \
                         tree outlived the close; {} sent by pid to {sent:?}",
                        watch.owner,
                        watch.term,
                        pids.len(),
                        if hard { "SIGKILL" } else { "SIGTERM" },
                    ));
                    match hard {
                        false => watch.polite_ms = Some(now_ms),
                        true => watch.hard_ms = Some(now_ms),
                    }
                    true
                }
                Step::GaveUp(pids) => {
                    lines.push(format!(
                        "{}'s released pane (terminal {}): {pids:?} still stand after \
                         SIGKILL; the window stops watching them",
                        watch.owner, watch.term,
                    ));
                    false
                }
            }
        });
        lines
    }
}

fn reaper() -> &'static Mutex<Reaper> {
    static REAPER: OnceLock<Mutex<Reaper>> = OnceLock::new();
    REAPER.get_or_init(|| Mutex::new(Reaper::default()))
}

/// This machine's process table as [`Row`]s; `None` when it cannot be read.
pub(crate) fn table() -> Option<Vec<Row>> {
    let sample = crate::resource_usage::enumerate_processes().ok()?;
    Some(
        sample
            .identified()
            .map(|row| (row.pid, row.ppid, row.started.to_string()))
            .collect(),
    )
}

/// The tree under the pane's child, read while it still stands — what the
/// release arm holds before it closes the pane.
pub(crate) fn tree_before_close(witness: &crate::agent_teams::ExitWitness) -> Vec<Named> {
    let tree = table()
        .map(|rows| tree_under(&rows, witness.group))
        .unwrap_or_default();
    if !tree.is_empty() {
        return tree;
    }
    // A table that could not be read still names the child itself.
    witness
        .started
        .as_ref()
        .map(|started| {
            vec![Named {
                pid: witness.group,
                started: started.clone(),
            }]
        })
        .unwrap_or_default()
}

/// Watch a pane the release — or a sleep — just closed.
pub(crate) fn watch(owner: &str, term: u32, tree: Vec<Named>, closed_ms: i64) {
    reaper()
        .lock()
        .unwrap_or_else(|held| held.into_inner())
        .watch(Watch::new(owner, term, tree, closed_ms));
}

/// End one pid of a released pane's tree. Unix only: Windows has no
/// hang-up for a close to miss, and its close is the kill.
fn signal_pid(pid: u32, hard: bool) -> bool {
    #[cfg(unix)]
    {
        let Ok(pid) = libc::pid_t::try_from(pid) else {
            return false;
        };
        if pid <= 1 {
            return false;
        }
        let signal = if hard { libc::SIGKILL } else { libc::SIGTERM };
        // SAFETY: a positive literal pid, read off this pane's own tree a
        // moment ago with the start identity it had when the pane stood.
        unsafe { libc::kill(pid, signal) == 0 }
    }
    #[cfg(not(unix))]
    {
        let _ = (pid, hard);
        false
    }
}

/// One beat of the watch, on the beat that already exists. Reads the process
/// table only while some released pane is watched.
pub(crate) fn sweep(now_ms: i64) {
    let mut held = reaper().lock().unwrap_or_else(|held| held.into_inner());
    if held.is_idle() {
        return;
    }
    let Some(rows) = table() else {
        return;
    };
    let lines = held.beat(&rows, now_ms, &mut signal_pid);
    drop(held);
    if let Some(root) = super::BLACKBOX.get() {
        for line in lines {
            crate::note_window_event(root, &format!("orchestration: {line}"));
        }
    }
}

#[cfg(test)]
mod tests;
