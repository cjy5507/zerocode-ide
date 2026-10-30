//! The agent processes a window's panes leave running when it goes
//! (t-76434): ended on the way out, written down for the next window, and
//! finished there only when the next window can prove a process is the last
//! one's own leftover.
//!
//! On 2026-10-26 22:56:34 the update toast restarted the window over four
//! coordinators mid-turn. The restart replaces the process and ended no
//! pane: the kernel's hangup reached each pane's session as its terminal
//! closed, and three zo processes went on turning without one — parent
//! launchd, no terminal, 35–40 % CPU each — for over an hour, each holding
//! its conversation's writer lease. The new window's two zo resumes died at
//! those leases (`term 4/7 refused zo: the pane exited (code 1)`), and the
//! person reopened the coordinator by hand once someone had sent the three a
//! TERM, which each answered by saving its session and leaving.
//!
//! Only a process this window launched as a pane's agent is ever signalled
//! ([`launched`]): its pid and the instant it began, read the moment the
//! window spawned it. A pane's shell, and whatever a person runs in front of
//! it — an editor opened after quitting an agent typed there — is never
//! named, whatever the pane's agent mark says (t-76504). Before every signal
//! the process is looked at again ([`standing`]): the same beginning, the
//! same executable it ran when it was written down, still leading its own
//! group, this user's — anything else, or a look that cannot say, is not
//! signalled.
//!
//! So the way out, on a thread of its own, collects those processes, writes
//! them down ([`LEFT_FILE`]), sends each one's group a hangup, a short grace
//! later a TERM, and at its deadline a KILL ([`end`]). The main thread waits
//! for the note and the hangup no longer than [`WRITE_PATIENCE`] and for
//! nothing after: a pane's lock or the disk held by a stuck thread costs the
//! way out that much and no more. A restart replaces the process within
//! milliseconds, so that thread may not see its deadline; whatever it did not
//! see end, the next window finds in the note. There a process is the last
//! window's only when it still stands as written down, its parent is
//! launchd, and it has no terminal ([`proven`]). Each one proven is sent a
//! TERM, then a KILL at the deadline ([`BOOT_PLAN`]) — by the boot's sweep,
//! or first by the wake that reopens its conversation
//! ([`free_conversation`]), so the resume no longer meets its lease. A zo
//! somebody opened in a terminal, or one holding another conversation, is
//! never proven. The note forgets a process only once it is seen gone; one
//! not proven, or not ended by the KILL, stays written down for the next
//! window to look at again.
//!
//! A terminal whose other end is gone counts as none (t-77414). Panes start
//! with their terminal as their controlling one, so the window going sends
//! each session a hangup; a leftover that ignored it still holds that
//! terminal, closed master and all, and it is the same leftover as one that
//! never had a terminal.
//!
//! Process groups and signals are Unix's; on Windows no group is collected
//! and nothing is signalled, and the pane's ConPTY closing is all that ends
//! its agent there, as before.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex, PoisonError};
use std::time::Duration;

/// Where the way out writes its pane groups, beside the window's log in the
/// local data root.
pub(crate) const LEFT_FILE: &str = "pane-agents-left.json";

/// The launchd pid: the parent every orphan is handed to on macOS.
const LAUNCHD: u32 = 1;

/// One pane's agent process group, as the way out found it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PaneGroup {
    pub(crate) term: u32,
    /// The group's leader, whose pid is the group's id: the agent the window
    /// launched as the pane's own child, which leads the pane's session.
    pub(crate) leader: u32,
    pub(crate) agent: String,
    /// The conversation the pane reported, when it reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session: Option<String>,
    /// When the leader began, in microseconds since the epoch — what tells
    /// it from a later process that took its pid. `None` where the host
    /// could not say: such a group is never signalled nor proven.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) started_us: Option<u64>,
    /// The executable the leader ran when it was written down. `None` where
    /// the host could not say: such a group is never signalled nor proven.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) exe: Option<String>,
}

impl PaneGroup {
    /// The words a log line names it by: pid, pane, agent, conversation.
    fn named(&self) -> String {
        format!(
            "pid {} (terminal {}, {}{})",
            self.leader,
            self.term,
            self.agent,
            self.session
                .as_deref()
                .map_or_else(String::new, |session| format!(", session {session}"))
        )
    }

    /// Whether it carries what [`standing`] needs to say it is the process
    /// written down — a group without is nobody's to end.
    fn identified(&self) -> bool {
        self.started_us.is_some() && self.exe.is_some()
    }
}

/// What the host says of one process right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Seen {
    pub(crate) parent: u32,
    /// Whether it has a controlling terminal that still has its other end:
    /// for a process handed to launchd, one whose master is closed — the
    /// window that held it gone — is none (`hung_up`).
    pub(crate) terminal: bool,
    pub(crate) started_us: u64,
    /// The process group it is in.
    pub(crate) group: u32,
    /// The user it runs as.
    pub(crate) owner: u32,
    /// Its executable, when the host could read it.
    pub(crate) exe: Option<String>,
}

/// One look at a pid. Only macOS reads a process
/// ([`looked_up_by_the_kernel`]); elsewhere every look is [`Lookup::Unread`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Lookup {
    /// No process has the pid.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Gone,
    /// A process has it, and the host could not say what it is.
    Unread,
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Seen(Seen),
}

/// What a written-down group is now, by one look at its leader.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Standing {
    /// Still the process written down: the same beginning, the same
    /// executable, its own group's leader, this user's.
    Ours,
    /// Seen gone: no process has the pid, or a later one does.
    Ended,
    /// Not known to be either — never signalled, and never forgotten.
    Unknown,
}

/// A signal [`end`] sends a group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Signal {
    Hangup,
    Terminate,
    Kill,
}

/// The host [`end`] and [`proven`] ask: the process table, the signals, and
/// a clock — the product's is [`System`], a test's a table and a clock of
/// its own, so no real process is ever signalled by a test.
pub(crate) trait Host {
    fn look(&self, pid: u32) -> Lookup;
    /// The user this window runs as.
    fn owner(&self) -> u32;
    /// Send `signal` to the process group `group`.
    fn signal_group(&self, group: u32, signal: Signal);
    /// Time since an origin of the host's own.
    fn elapsed(&self) -> Duration;
    fn wait(&self, span: Duration);
}

/// What `group` is now ([`Standing`]), by one look at its leader.
pub(crate) fn standing(group: &PaneGroup, host: &dyn Host) -> Standing {
    standing_of(group, &host.look(group.leader), host.owner())
}

fn standing_of(group: &PaneGroup, look: &Lookup, owner: u32) -> Standing {
    let (Some(started), Some(exe)) = (group.started_us, group.exe.as_deref()) else {
        return Standing::Unknown;
    };
    match look {
        Lookup::Gone => Standing::Ended,
        Lookup::Unread => Standing::Unknown,
        Lookup::Seen(seen) if seen.started_us != started => Standing::Ended,
        Lookup::Seen(seen)
            if seen.exe.as_deref() == Some(exe)
                && seen.group == group.leader
                && seen.owner == owner =>
        {
            Standing::Ours
        }
        Lookup::Seen(_) => Standing::Unknown,
    }
}

/// How [`end`] goes about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Plan {
    /// Whether it begins with a hangup and its grace, or straight at TERM.
    pub(crate) hangup: bool,
    /// How long after the hangup the TERM goes.
    pub(crate) grace: Duration,
    /// How long after the first signal the KILL goes.
    pub(crate) deadline: Duration,
    /// How long it looks for the KILL to land before naming what is left.
    pub(crate) after_kill: Duration,
    /// How often it looks while it waits.
    pub(crate) poll: Duration,
}

/// The way out's plan: a hangup first — what the closing terminal sends
/// anyway, and what a CLI that saves on hangup answers — a TERM 200 ms on,
/// a KILL at two seconds. zo answered TERM at 23:0x by saving its session
/// and leaving.
pub(crate) const EXIT_PLAN: Plan = Plan {
    hangup: true,
    grace: Duration::from_millis(200),
    deadline: Duration::from_secs(2),
    after_kill: Duration::from_millis(200),
    poll: Duration::from_millis(25),
};

/// The next window's plan for a proven leftover: no terminal is left to hang
/// up, so a TERM, and a KILL five seconds on — long enough for a turn's
/// session to be written, short enough that the resume waiting on it is
/// still a resume (its own channel waits twenty).
pub(crate) const BOOT_PLAN: Plan = Plan {
    hangup: false,
    grace: Duration::ZERO,
    deadline: Duration::from_secs(5),
    after_kill: Duration::from_millis(500),
    poll: Duration::from_millis(50),
};

/// How long the way out's main thread waits for the note to be written and
/// the first signal sent (t-76504): the screens' own capture waits 300 ms on
/// the same kind of locks, and a note that needs longer is a lock or a disk
/// held by a stuck thread, which the way out does not wait on.
pub(crate) const WRITE_PATIENCE: Duration = Duration::from_millis(150);

/// End `groups` by `plan`, and answer the ones not seen to end. Each signal
/// goes only to a group whose leader is, at that moment, still the process
/// written down ([`standing`]); `sent` is told once the first round has
/// gone.
pub(crate) fn end(
    groups: Vec<PaneGroup>,
    host: &dyn Host,
    plan: Plan,
    sent: &mut dyn FnMut(),
) -> Vec<PaneGroup> {
    let mut live: Vec<PaneGroup> = groups
        .into_iter()
        .filter(|group| standing(group, host) != Standing::Ended)
        .collect();
    let from = host.elapsed();
    if plan.hangup {
        send(&live, host, Signal::Hangup);
        sent();
        wait_out(&mut live, host, from + plan.grace, plan.poll);
    }
    send(&live, host, Signal::Terminate);
    if !plan.hangup {
        sent();
    }
    wait_out(&mut live, host, from + plan.deadline, plan.poll);
    send(&live, host, Signal::Kill);
    let killed = host.elapsed();
    wait_out(&mut live, host, killed + plan.after_kill, plan.poll);
    live
}

/// Send `signal` to each group in `live` that is, by a look taken just
/// before, still the process written down — never on an older look.
fn send(live: &[PaneGroup], host: &dyn Host, signal: Signal) {
    for group in live {
        if standing(group, host) == Standing::Ours {
            host.signal_group(group.leader, signal);
        }
    }
}

/// Look until every group in `live` is seen gone or `until` comes.
fn wait_out(live: &mut Vec<PaneGroup>, host: &dyn Host, until: Duration, poll: Duration) {
    loop {
        live.retain(|group| standing(group, host) != Standing::Ended);
        let now = host.elapsed();
        if live.is_empty() || now >= until {
            return;
        }
        host.wait(poll.min(until - now));
    }
}

/// Whether `group`, written down by the last window's way out, is that
/// window's leftover: still the process it was ([`Standing::Ours`]), handed
/// to launchd, and holding no terminal. A group whose beginning or
/// executable was never read is never proven.
pub(crate) fn proven(group: &PaneGroup, host: &dyn Host) -> bool {
    let look = host.look(group.leader);
    standing_of(group, &look, host.owner()) == Standing::Ours
        && matches!(look, Lookup::Seen(seen) if seen.parent == LAUNCHD && !seen.terminal)
}

/// The log line for a group the way out could not end.
pub(crate) fn left_line(group: &PaneGroup) -> String {
    format!(
        "exit: a pane's agent did not end within {} ms — {}",
        EXIT_PLAN.deadline.as_millis() + EXIT_PLAN.after_kill.as_millis(),
        group.named()
    )
}

/// Write `groups` down for the next window, in one atomic replace. Nothing
/// to write removes any older note.
pub(crate) fn write_left(root: &Path, groups: &[PaneGroup]) -> std::io::Result<()> {
    let path = root.join(LEFT_FILE);
    if groups.is_empty() {
        return match crate::durable_file::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let bytes = serde_json::to_vec(groups).map_err(std::io::Error::other)?;
    crate::durable_file::replace_bytes(&path, &bytes).map(|_| ())
}

/// One agent the window launched as a pane's own child: its pid and when it
/// began, read the moment it was spawned, and which agent it is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Launch {
    pub(crate) pid: u32,
    pub(crate) started_us: u64,
    pub(crate) agent: String,
}

/// Every pane's launched agent, by terminal.
static LAUNCHES: Mutex<Vec<(u32, Launch)>> = Mutex::new(Vec::new());

/// The window spawned `agent` as `term`'s own child, `pid`: the one process
/// of that pane the way out may end. Read now, while the pid is certainly
/// the child's; a pid the host cannot read is not written down, and so is
/// never ended. Launches seen gone are dropped as each new one comes.
pub(crate) fn launched(term: u32, pid: Option<u32>, agent: &str) {
    let Some(pid) = pid else {
        return;
    };
    let host = System::new();
    let Lookup::Seen(seen) = host.look(pid) else {
        return;
    };
    let mut held = LAUNCHES.lock().unwrap_or_else(PoisonError::into_inner);
    held.retain(|(was, launch)| {
        *was != term
            && match host.look(launch.pid) {
                Lookup::Gone => false,
                Lookup::Seen(seen) => seen.started_us == launch.started_us,
                Lookup::Unread => true,
            }
    });
    held.push((
        term,
        Launch {
            pid,
            started_us: seen.started_us,
            agent: agent.to_string(),
        },
    ));
}

/// What the window launched as `term`'s own child, as it was written down
/// ([`launched`]): the pid and the agent. `None` for a pane this window
/// spawned no agent into — a shell, whatever runs in front of it now.
pub(crate) fn launched_as(term: u32) -> Option<(u32, String)> {
    LAUNCHES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .iter()
        .rev()
        .find(|(was, _)| *was == term)
        .map(|(_, launch)| (launch.pid, launch.agent.clone()))
}

/// The groups the way out ends: each launched agent that is still the
/// process the window spawned — the same pid and beginning, leading its own
/// group, this user's — with the executable it runs now, which every later
/// signal and the next window's proof hold it to. Nothing else of any pane.
pub(crate) fn launched_groups(
    launches: &[(u32, Launch)],
    session_of: &dyn Fn(u32) -> Option<String>,
    host: &dyn Host,
) -> Vec<PaneGroup> {
    launches
        .iter()
        .filter_map(|(term, launch)| {
            let Lookup::Seen(seen) = host.look(launch.pid) else {
                return None;
            };
            if seen.started_us != launch.started_us
                || seen.group != launch.pid
                || seen.owner != host.owner()
            {
                return None;
            }
            Some(PaneGroup {
                term: *term,
                leader: launch.pid,
                agent: launch.agent.clone(),
                session: session_of(*term),
                started_us: Some(launch.started_us),
                exe: Some(seen.exe?),
            })
        })
        .collect()
}

/// The window's own pane groups, read for the way out — on the way out's
/// own thread, never the main one ([`leave`]).
pub(crate) fn pane_groups(state: &crate::AppState) -> Vec<PaneGroup> {
    use crate::ShellStateExt as _;
    // No process groups to signal on Windows: nothing is collected there.
    if cfg!(not(unix)) {
        return Vec::new();
    }
    let launches = LAUNCHES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    if launches.is_empty() {
        return Vec::new();
    }
    let sessions: HashMap<u32, String> = state
        .pane_sessions()
        .iter()
        .map(|(term, session)| (*term, session.id.clone()))
        .collect();
    launched_groups(
        &launches,
        &|term| sessions.get(&term).cloned(),
        &System::new(),
    )
}

/// The way out's step: on a thread of its own, collect the panes' agents,
/// write them down, and end them by [`EXIT_PLAN`], naming in the log each
/// one left and writing down only those for the next window — if this
/// process lives that long. The main thread waits for the note and the
/// hangup at most [`WRITE_PATIENCE`].
pub(crate) fn leave(
    root: PathBuf,
    collect: impl FnOnce() -> Vec<PaneGroup> + Send + 'static,
    note: fn(&Path, &str),
) {
    leave_within(&LEFTOVERS, root, collect, System::new, note, WRITE_PATIENCE);
}

/// [`leave`] with its book, host and patience named: answers whether the
/// note was written and the first signal sent within `patience`.
fn leave_within<H: Host + 'static>(
    book: &'static Leftovers,
    root: PathBuf,
    collect: impl FnOnce() -> Vec<PaneGroup> + Send + 'static,
    host: fn() -> H,
    note: fn(&Path, &str),
    patience: Duration,
) -> bool {
    let (told, heard) = std::sync::mpsc::sync_channel(1);
    let spawned = std::thread::Builder::new()
        .name("zerocode-pane-hangup".to_string())
        .spawn(move || {
            let groups = collect();
            let host = host();
            if let Err(error) = book.keep_own(&root, &groups) {
                note(
                    &root,
                    &format!("exit: the pane agents were not written down: {error}"),
                );
            }
            let left = end(groups, &host, EXIT_PLAN, &mut || {
                let _ = told.try_send(());
            });
            for group in &left {
                note(&root, &left_line(group));
            }
            let _ = book.keep_own(&root, &left);
        });
    if let Err(error) = spawned {
        eprintln!("zerocode-shell: the pane hangup thread did not start: {error}");
        return false;
    }
    heard.recv_timeout(patience).is_ok()
}

/// What the last window wrote down, as this process holds it: each group
/// waiting, being ended, or done with — and what this window's own way out
/// writes down beside it.
#[derive(Debug, Default)]
struct Book {
    read: bool,
    groups: Vec<(PaneGroup, Phase)>,
    own: Vec<PaneGroup>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Waiting,
    Ending,
    Done,
}

impl Book {
    /// Read the last window's note off the disk, once per process. The file
    /// stays: it is rewritten as groups are seen gone ([`Book::write`]), so
    /// a window that dies mid-way leaves the next one the rest.
    fn read_once(&mut self, root: &Path) {
        if std::mem::replace(&mut self.read, true) {
            return;
        }
        let groups: Vec<PaneGroup> = crate::durable_file::read_plain_file(&root.join(LEFT_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        self.groups = groups
            .into_iter()
            .filter(PaneGroup::identified)
            .map(|group| (group, Phase::Waiting))
            .collect();
    }

    /// The note as it stands: this window's own groups, and every one of the
    /// last window's not yet seen gone.
    fn write(&self, root: &Path) -> std::io::Result<()> {
        let mut all = self.own.clone();
        for (group, phase) in &self.groups {
            if *phase != Phase::Done && !all.contains(group) {
                all.push(group.clone());
            }
        }
        write_left(root, &all)
    }
}

/// The process-wide book and the bell rung whenever a group is done with.
pub(crate) struct Leftovers {
    book: Mutex<Book>,
    done: Condvar,
}

impl Leftovers {
    pub(crate) const fn new() -> Self {
        Self {
            book: Mutex::new(Book {
                read: false,
                groups: Vec::new(),
                own: Vec::new(),
            }),
            done: Condvar::new(),
        }
    }

    /// This window's own groups written down, beside whatever of the last
    /// window's is not yet seen gone — read first, so the way out never
    /// writes over a note no road of this window has read.
    fn keep_own(&self, root: &Path, own: &[PaneGroup]) -> std::io::Result<()> {
        let mut book = self.book.lock().unwrap_or_else(PoisonError::into_inner);
        book.read_once(root);
        book.own = own.to_vec();
        book.write(root)
    }

    /// Take the waiting groups `wanted` picks for ending, and wait out any
    /// it picks that another road is ending now.
    fn take(&self, root: &Path, wanted: impl Fn(&PaneGroup) -> bool) -> Vec<PaneGroup> {
        let mut book = self.book.lock().unwrap_or_else(PoisonError::into_inner);
        book.read_once(root);
        loop {
            if !book
                .groups
                .iter()
                .any(|(group, phase)| *phase == Phase::Ending && wanted(group))
            {
                break;
            }
            let bound = BOOT_PLAN.deadline + BOOT_PLAN.after_kill + BOOT_PLAN.poll;
            let (held, timeout) = self
                .done
                .wait_timeout(book, bound)
                .unwrap_or_else(PoisonError::into_inner);
            book = held;
            if timeout.timed_out() {
                break;
            }
        }
        book.groups
            .iter_mut()
            .filter(|(group, phase)| *phase == Phase::Waiting && wanted(group))
            .map(|(group, phase)| {
                *phase = Phase::Ending;
                group.clone()
            })
            .collect()
    }

    /// The groups `taken` settled: those in `ended` done with, the rest
    /// waiting again — and the note rewritten to match.
    fn finish(&self, root: &Path, taken: &[PaneGroup], ended: &[PaneGroup]) {
        let mut book = self.book.lock().unwrap_or_else(PoisonError::into_inner);
        for (group, phase) in &mut book.groups {
            if taken.contains(group) {
                *phase = if ended.contains(group) {
                    Phase::Done
                } else {
                    Phase::Waiting
                };
            }
        }
        let _ = book.write(root);
        drop(book);
        self.done.notify_all();
    }

    /// End every group `wanted` picks that [`proven`] holds for, by
    /// [`BOOT_PLAN`], and say each one in the log: ended, or left. A group
    /// is forgotten only once seen gone; one not proven, or left after the
    /// KILL, stays written down.
    fn end_proven(
        &self,
        root: &Path,
        host: &dyn Host,
        wanted: impl Fn(&PaneGroup) -> bool,
        note: &dyn Fn(&str),
    ) {
        let taken = self.take(root, wanted);
        if taken.is_empty() {
            return;
        }
        let mut ended = Vec::new();
        let mut ours = Vec::new();
        for group in &taken {
            if standing(group, host) == Standing::Ended {
                ended.push(group.clone());
            } else if proven(group, host) {
                ours.push(group.clone());
            }
        }
        let left = end(ours.clone(), host, BOOT_PLAN, &mut || {});
        for group in ours {
            if left.contains(&group) {
                note(&format!(
                    "boot: the last window's pane agent did not end on TERM or KILL — {}",
                    group.named()
                ));
            } else {
                note(&format!(
                    "boot: ended the last window's pane agent left without a terminal — {}",
                    group.named()
                ));
                ended.push(group);
            }
        }
        self.finish(root, &taken, &ended);
    }
}

/// This process's book: one window, one last window.
static LEFTOVERS: Leftovers = Leftovers::new();

/// The boot's sweep: every proven leftover of the last window, ended on a
/// thread of its own.
pub(crate) fn sweep_at_boot(root: PathBuf, note: fn(&Path, &str)) {
    let spawned = std::thread::Builder::new()
        .name("zerocode-pane-leftovers".to_string())
        .spawn(move || {
            LEFTOVERS.end_proven(&root, &System::new(), |_| true, &|line| note(&root, line));
        });
    if let Err(error) = spawned {
        eprintln!("zerocode-shell: the pane leftover sweep did not start: {error}");
    }
}

/// Before a wake reopens `session`: the last window's proven leftover
/// holding that very conversation is ended first — by this road, or by the
/// boot's sweep while this road waits for it — so the resume does not meet
/// its writer lease. Returns once nothing of the last window holds it.
pub(crate) fn free_conversation(root: &Path, agent: &str, session: &str, note: &dyn Fn(&str)) {
    LEFTOVERS.end_proven(
        root,
        &System::new(),
        |group| group.agent == agent && group.session.as_deref() == Some(session),
        note,
    );
}

/// The product's host: the kernel's own answers, and the wall of real time.
pub(crate) struct System {
    origin: std::time::Instant,
}

impl System {
    pub(crate) fn new() -> Self {
        Self {
            origin: std::time::Instant::now(),
        }
    }
}

impl Host for System {
    fn look(&self, pid: u32) -> Lookup {
        looked_up_by_the_kernel(pid)
    }

    fn owner(&self) -> u32 {
        own_user()
    }

    fn signal_group(&self, group: u32, signal: Signal) {
        signal_group(group, signal);
    }

    fn elapsed(&self) -> Duration {
        self.origin.elapsed()
    }

    fn wait(&self, span: Duration) {
        std::thread::sleep(span);
    }
}

/// One process as macOS's `proc_pidinfo` says it — parent, controlling
/// terminal, beginning, group and user in one call — and `proc_pidpath` its
/// executable; no child process.
#[cfg(target_os = "macos")]
fn looked_up_by_the_kernel(pid: u32) -> Lookup {
    let Some(pid) = libc::c_int::try_from(pid).ok().filter(|pid| *pid > 0) else {
        return Lookup::Unread;
    };
    let Ok(size) = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()) else {
        return Lookup::Unread;
    };
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    // SAFETY: `proc_pidinfo` writes at most `size` bytes into `info` and
    // answers how many it wrote; the struct is read only when it wrote all.
    let wrote = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast::<libc::c_void>(),
            size,
        )
    };
    if wrote != size {
        // Only ESRCH proves absence; anything else is a process the host
        // would not describe.
        // SAFETY: probing with signal 0 sends nothing.
        let gone = unsafe { libc::kill(pid, 0) } != 0
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH);
        return if gone { Lookup::Gone } else { Lookup::Unread };
    }
    // SAFETY: the call above filled the whole struct.
    let info = unsafe { info.assume_init() };
    let mut path = vec![0u8; libc::PROC_PIDPATHINFO_MAXSIZE as usize];
    // SAFETY: `proc_pidpath` writes at most the buffer's length and answers
    // how many bytes it wrote.
    let len = unsafe {
        libc::proc_pidpath(
            pid,
            path.as_mut_ptr().cast::<libc::c_void>(),
            u32::try_from(path.len()).unwrap_or(0),
        )
    };
    let exe = usize::try_from(len)
        .ok()
        .filter(|len| *len > 0)
        .map(|len| String::from_utf8_lossy(&path[..len]).into_owned());
    // `e_tdev` is NODEV (all bits set) for a process with no terminal. Only
    // an orphan's terminal is asked whether it hung up: that is the one
    // question the answer serves ([`proven`]), and a live pane's terminal is
    // not opened from here.
    let has_terminal = info.e_tdev != u32::MAX && info.e_tdev != 0;
    Lookup::Seen(Seen {
        parent: info.pbi_ppid,
        terminal: has_terminal && !(info.pbi_ppid == LAUNCHD && hung_up(info.e_tdev)),
        started_us: info
            .pbi_start_tvsec
            .saturating_mul(1_000_000)
            .saturating_add(info.pbi_start_tvusec),
        group: info.pbi_pgid,
        owner: info.pbi_uid,
        exe,
    })
}

/// Whether the terminal device `dev` has lost its other end (t-77414): its
/// master closed. Opened by name with no claim on it (`O_NOCTTY`) and asked
/// once without waiting; only such a terminal answers `POLLHUP`, and one the
/// host will not name or open is not said to have hung up.
#[cfg(target_os = "macos")]
fn hung_up(dev: u32) -> bool {
    unsafe extern "C" {
        fn devname_r(
            dev: libc::dev_t,
            kind: libc::mode_t,
            buf: *mut libc::c_char,
            len: libc::c_int,
        ) -> *mut libc::c_char;
    }
    let mut name = [0 as libc::c_char; 64];
    let Ok(len) = libc::c_int::try_from(name.len()) else {
        return false;
    };
    // SAFETY: `devname_r` writes at most `len` bytes into `name`, NUL
    // included, and answers null or a pointer into it.
    let named = unsafe {
        devname_r(
            libc::dev_t::from_ne_bytes(dev.to_ne_bytes()),
            libc::S_IFCHR,
            name.as_mut_ptr(),
            len,
        )
    };
    if named.is_null() {
        return false;
    }
    // SAFETY: a non-null answer is a NUL-terminated name inside `name`.
    let name = unsafe { std::ffi::CStr::from_ptr(named) };
    let mut path = b"/dev/".to_vec();
    path.extend_from_slice(name.to_bytes());
    let Ok(path) = std::ffi::CString::new(path) else {
        return false;
    };
    // SAFETY: a path open; the descriptor is closed below.
    let fd = unsafe {
        libc::open(
            path.as_ptr(),
            libc::O_RDONLY | libc::O_NOCTTY | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if fd < 0 {
        return false;
    }
    let mut asked = libc::pollfd {
        fd,
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: one pollfd, no wait.
    let answered = unsafe { libc::poll(&raw mut asked, 1, 0) };
    // SAFETY: the descriptor opened above, closed once.
    unsafe { libc::close(fd) };
    answered > 0 && asked.revents & libc::POLLHUP != 0
}

/// Elsewhere the window reads no process, so nothing is ever collected,
/// signalled or proven.
#[cfg(not(target_os = "macos"))]
fn looked_up_by_the_kernel(_pid: u32) -> Lookup {
    Lookup::Unread
}

#[cfg(unix)]
pub(crate) fn own_user() -> u32 {
    // SAFETY: `getuid` takes nothing and cannot fail.
    unsafe { libc::getuid() }
}

#[cfg(not(unix))]
pub(crate) fn own_user() -> u32 {
    0
}

#[cfg(unix)]
fn signal_group(group: u32, signal: Signal) {
    let number = match signal {
        Signal::Hangup => libc::SIGHUP,
        Signal::Terminate => libc::SIGTERM,
        Signal::Kill => libc::SIGKILL,
    };
    if group > LAUNCHD {
        let _ = crate::codex_queue::signal_process_group(group, number);
    }
}

#[cfg(not(unix))]
fn signal_group(_group: u32, _signal: Signal) {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    /// The executable every fake agent runs.
    const ZO: &str = "/Users/dev/.local/bin/zo";
    /// The user every fake process runs as, and the window's own.
    const ME: u32 = 501;

    /// A fake process: what the table says of it, and what it does with each
    /// signal — `ends_on` the first signal it leaves on, `None` for one that
    /// nothing but a KILL ends (and not even that when `unkillable`). A
    /// `successor` takes its pid the instant it ends; an `unread_after`
    /// process can no longer be described once that signal has reached it.
    #[derive(Clone)]
    struct Fake {
        seen: Seen,
        ends_on: Option<Signal>,
        unkillable: bool,
        alive: bool,
        unread: bool,
        unread_after: Option<Signal>,
        successor: Option<Box<Fake>>,
    }

    /// A process table and a clock of its own: `wait` moves the clock, and
    /// every signal is written down in order. With `note_at`, every wait
    /// also reads the note on the disk — what a window dying at that moment
    /// would leave the next one.
    struct Table {
        procs: RefCell<HashMap<u32, Fake>>,
        now: RefCell<Duration>,
        sent: RefCell<Vec<(u32, Signal, Duration)>>,
        note_at: Option<PathBuf>,
        notes: RefCell<Vec<Vec<PaneGroup>>>,
    }

    impl Table {
        fn of(procs: &[(u32, Fake)]) -> Self {
            Self {
                procs: RefCell::new(procs.iter().cloned().collect()),
                now: RefCell::new(Duration::ZERO),
                sent: RefCell::new(Vec::new()),
                note_at: None,
                notes: RefCell::new(Vec::new()),
            }
        }

        fn signals_to(&self, pid: u32) -> Vec<Signal> {
            self.sent
                .borrow()
                .iter()
                .filter(|(to, _, _)| *to == pid)
                .map(|(_, signal, _)| *signal)
                .collect()
        }

        fn alive(&self, pid: u32) -> bool {
            self.procs.borrow().get(&pid).is_some_and(|one| one.alive)
        }

        fn begun(&self, pid: u32) -> Option<u64> {
            self.procs
                .borrow()
                .get(&pid)
                .filter(|one| one.alive)
                .map(|one| one.seen.started_us)
        }
    }

    impl Host for Table {
        fn look(&self, pid: u32) -> Lookup {
            match self.procs.borrow().get(&pid) {
                Some(one) if one.alive && one.unread => Lookup::Unread,
                Some(one) if one.alive => Lookup::Seen(one.seen.clone()),
                _ => Lookup::Gone,
            }
        }

        fn owner(&self) -> u32 {
            ME
        }

        fn signal_group(&self, group: u32, signal: Signal) {
            self.sent
                .borrow_mut()
                .push((group, signal, *self.now.borrow()));
            let mut procs = self.procs.borrow_mut();
            let Some(one) = procs.get_mut(&group).filter(|one| one.alive) else {
                return;
            };
            let ends = match signal {
                Signal::Kill => !one.unkillable,
                _ => one.ends_on == Some(signal),
            };
            if one.unread_after == Some(signal) {
                one.unread = true;
            }
            if ends {
                one.alive = false;
                if let Some(next) = one.successor.take() {
                    *one = *next;
                }
            }
        }

        fn elapsed(&self) -> Duration {
            *self.now.borrow()
        }

        fn wait(&self, span: Duration) {
            *self.now.borrow_mut() += span;
            if let Some(root) = &self.note_at {
                let note = std::fs::read(root.join(LEFT_FILE))
                    .ok()
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok())
                    .unwrap_or_default();
                self.notes.borrow_mut().push(note);
            }
        }
    }

    const BORN: u64 = 1_793_000_000_000_000;

    /// A process the window launched as pane agent `pid`, now an orphan:
    /// parent launchd, no terminal, leading its own group.
    fn orphan_at(pid: u32, ends_on: Option<Signal>) -> (u32, Fake) {
        (
            pid,
            Fake {
                seen: Seen {
                    parent: LAUNCHD,
                    terminal: false,
                    started_us: BORN,
                    group: pid,
                    owner: ME,
                    exe: Some(ZO.to_string()),
                },
                ends_on,
                unkillable: false,
                alive: true,
                unread: false,
                unread_after: None,
                successor: None,
            },
        )
    }

    fn group(term: u32, leader: u32, session: &str) -> PaneGroup {
        PaneGroup {
            term,
            leader,
            agent: "zo".to_string(),
            session: Some(session.to_string()),
            started_us: Some(BORN),
            exe: Some(ZO.to_string()),
        }
    }

    /// The way out ends every pane's group in its deadline: one that leaves
    /// on the hangup gets nothing more, one that ignores the hangup gets a
    /// TERM at the grace, one that ignores both gets a KILL at the deadline —
    /// and none of it outlasts the plan.
    #[test]
    fn the_way_out_ends_a_child_that_ignores_the_hangup_within_its_deadline() {
        let table = Table::of(&[
            orphan_at(60171, Some(Signal::Hangup)),
            orphan_at(60693, Some(Signal::Terminate)),
            orphan_at(60955, None),
        ]);
        let mut first = Vec::new();
        let left = end(
            vec![
                group(4, 60171, "s-1"),
                group(7, 60693, "s-2"),
                group(9, 60955, "s-3"),
            ],
            &table,
            EXIT_PLAN,
            &mut || first.push(table.sent.borrow().len()),
        );
        assert!(left.is_empty(), "{left:?}");
        assert_eq!(first, [3], "told once, after the three hangups");
        assert_eq!(table.signals_to(60171), vec![Signal::Hangup]);
        assert_eq!(
            table.signals_to(60693),
            vec![Signal::Hangup, Signal::Terminate]
        );
        assert_eq!(
            table.signals_to(60955),
            vec![Signal::Hangup, Signal::Terminate, Signal::Kill]
        );
        let at = |pid: u32, signal: Signal| {
            table
                .sent
                .borrow()
                .iter()
                .find(|(to, sent, _)| *to == pid && *sent == signal)
                .map(|(_, _, at)| *at)
        };
        assert_eq!(at(60693, Signal::Terminate), Some(EXIT_PLAN.grace));
        assert_eq!(at(60955, Signal::Kill), Some(EXIT_PLAN.deadline));
        assert!(
            table.elapsed() <= EXIT_PLAN.deadline + EXIT_PLAN.after_kill,
            "the way out waited {:?}",
            table.elapsed()
        );
    }

    /// A group nothing ends is answered, and its line names the pid, the
    /// pane, the agent and the conversation.
    #[test]
    fn a_group_that_does_not_end_is_named() {
        let (pid, mut stuck) = orphan_at(60955, None);
        stuck.unkillable = true;
        let table = Table::of(&[(pid, stuck)]);
        let left = end(vec![group(9, 60955, "s-3")], &table, EXIT_PLAN, &mut || {});
        assert_eq!(left, vec![group(9, 60955, "s-3")]);
        assert_eq!(
            left_line(&left[0]),
            "exit: a pane's agent did not end within 2200 ms — pid 60955 (terminal 9, zo, \
             session s-3)"
        );
    }

    /// A pid a later process took is not the group written down: nothing is
    /// sent to it.
    #[test]
    fn a_later_process_on_the_same_pid_is_never_signalled() {
        let (pid, mut later) = orphan_at(60171, None);
        later.seen.started_us = BORN + 1;
        let table = Table::of(&[(pid, later)]);
        assert!(end(vec![group(4, 60171, "s-1")], &table, EXIT_PLAN, &mut || {}).is_empty());
        assert!(table.sent.borrow().is_empty());
        assert!(table.alive(60171));
    }

    /// t-76504 F2: the pid is looked at again before every signal, not only
    /// before the first. A group that leaves on the hangup, its pid taken at
    /// once by a later process leading a group of its own, gets no TERM and
    /// no KILL; one that leaves on the TERM, its pid taken likewise, gets no
    /// KILL; and the later processes are never touched. A leader the host
    /// can no longer describe gets nothing more either, and is answered as
    /// not seen to end.
    #[test]
    fn a_pid_taken_during_the_grace_is_never_signalled() {
        let later = |pid: u32| {
            let (_, mut next) = orphan_at(pid, None);
            next.seen.started_us = BORN + 9;
            next.seen.exe = Some("/usr/bin/vim".to_string());
            Box::new(next)
        };
        let (on_hangup, mut first) = orphan_at(60171, Some(Signal::Hangup));
        first.successor = Some(later(on_hangup));
        let (on_term, mut second) = orphan_at(60693, Some(Signal::Terminate));
        second.successor = Some(later(on_term));
        let (blind, mut third) = orphan_at(60955, None);
        third.unread_after = Some(Signal::Hangup);
        let table = Table::of(&[(on_hangup, first), (on_term, second), (blind, third)]);
        let left = end(
            vec![
                group(4, on_hangup, "s-1"),
                group(7, on_term, "s-2"),
                group(9, blind, "s-3"),
            ],
            &table,
            EXIT_PLAN,
            &mut || {},
        );
        assert_eq!(table.signals_to(on_hangup), vec![Signal::Hangup]);
        assert_eq!(
            table.signals_to(on_term),
            vec![Signal::Hangup, Signal::Terminate]
        );
        assert_eq!(table.signals_to(blind), vec![Signal::Hangup]);
        assert_eq!(
            table.begun(on_hangup),
            Some(BORN + 9),
            "the later process lives"
        );
        assert_eq!(
            table.begun(on_term),
            Some(BORN + 9),
            "the later process lives"
        );
        assert_eq!(left, vec![group(9, blind, "s-3")]);

        let (pid, mut leftover) = orphan_at(60171, None);
        leftover.ends_on = Some(Signal::Terminate);
        leftover.successor = Some(later(pid));
        let table = Table::of(&[(pid, leftover)]);
        assert!(end(vec![group(4, pid, "s-1")], &table, BOOT_PLAN, &mut || {}).is_empty());
        assert_eq!(table.signals_to(pid), vec![Signal::Terminate]);
    }

    /// The next boot's proof: the note names it, the same beginning, the same
    /// executable, its own group's leader, this user's, parent launchd, no
    /// terminal. A zo a person opened in a terminal, one whose parent is
    /// alive, one whose pid a later process took, one now running another
    /// executable, one moved out of its group, another user's, and one whose
    /// beginning or executable was never read are never proven.
    #[test]
    fn only_the_last_windows_own_orphan_is_proven() {
        let shaped = |pid: u32, shape: fn(&mut Seen)| {
            let (pid, mut one) = orphan_at(pid, None);
            shape(&mut one.seen);
            (pid, one)
        };
        let table = Table::of(&[
            orphan_at(60171, None),
            shaped(70001, |seen| seen.terminal = true),
            shaped(70002, |seen| seen.parent = 4242),
            shaped(70003, |seen| seen.started_us = BORN + 7),
            shaped(70004, |seen| seen.exe = Some("/usr/bin/vim".to_string())),
            shaped(70005, |seen| seen.group = 4242),
            shaped(70006, |seen| seen.owner = 0),
            shaped(70007, |seen| seen.exe = None),
        ]);
        assert!(proven(&group(4, 60171, "s-1"), &table));
        for pid in 70001..=70007 {
            assert!(!proven(&group(4, pid, "s-1"), &table), "{pid} was proven");
        }
        let unread = PaneGroup {
            started_us: None,
            ..group(4, 60171, "s-1")
        };
        assert!(
            !proven(&unread, &table),
            "a beginning never read was proven"
        );
        let no_exe = PaneGroup {
            exe: None,
            ..group(4, 60171, "s-1")
        };
        assert!(
            !proven(&no_exe, &table),
            "an executable never read was proven"
        );
        assert!(
            !proven(&group(4, 99999, "s-1"), &table),
            "a gone pid was proven"
        );
    }

    /// t-76504 F1, as the reviewer put it: a person ran an agent in a plain
    /// shell pane, quit it, and opened an editor there — the pane still
    /// marked as the agent's. Only what the window launched as a pane's
    /// agent is collected: the shell and the editor in front of it never
    /// are, and neither is a launched agent whose pid a later process took,
    /// one no longer leading its own group, another user's, or one whose
    /// executable cannot be read. A launched agent still standing is
    /// collected with the executable it runs now and its conversation.
    #[test]
    fn a_shell_pane_and_whatever_runs_in_front_of_it_are_never_collected() {
        let (shell, mut zsh) = orphan_at(41000, None);
        zsh.seen.exe = Some("/bin/zsh".to_string());
        zsh.seen.terminal = true;
        let (editor, mut vim) = orphan_at(41007, None);
        vim.seen.exe = Some("/usr/bin/vim".to_string());
        vim.seen.terminal = true;
        let shaped = |pid: u32, shape: fn(&mut Seen)| {
            let (pid, mut one) = orphan_at(pid, None);
            shape(&mut one.seen);
            (pid, one)
        };
        let table = Table::of(&[
            (shell, zsh),
            (editor, vim),
            orphan_at(60693, None),
            shaped(60694, |seen| seen.started_us = BORN + 3),
            shaped(60695, |seen| seen.group = 41000),
            shaped(60696, |seen| seen.owner = 0),
            shaped(60697, |seen| seen.exe = None),
        ]);
        let launch = |pid: u32| Launch {
            pid,
            started_us: BORN,
            agent: "zo".to_string(),
        };
        let launches = [
            (4, launch(60693)),
            (5, launch(60694)),
            (6, launch(60695)),
            (7, launch(60696)),
            (8, launch(60697)),
            (9, launch(60698)),
        ];
        let session_of = |term: u32| (term == 4).then(|| "coordinator".to_string());
        assert_eq!(
            launched_groups(&launches, &session_of, &table),
            vec![group(4, 60693, "coordinator")]
        );
        assert!(
            launched_groups(&[], &session_of, &table).is_empty(),
            "the shell pane's processes were collected"
        );
        // The product's collection reads launches and nothing else of a pane.
        let source = include_str!("pane_leftovers.rs");
        let body = source
            .split("pub(crate) fn pane_groups(")
            .nth(1)
            .and_then(|rest| rest.split("\n}\n").next())
            .expect("pane_groups");
        assert!(body.contains("launched_groups("), "{body}");
        for foreign in ["foreground_process_id", "agent_terms", ".pid()"] {
            assert!(!body.contains(foreign), "pane_groups reads {foreign}");
        }
    }

    /// t-76434, the next boot: the last window wrote down three zo panes. A
    /// wake reopening one conversation ends that conversation's proven
    /// orphan — TERM, and KILL at the deadline when TERM is not enough — and
    /// touches neither another conversation's orphan nor a process that is
    /// not proven; the boot's sweep then ends the rest that are proven, and
    /// nothing twice. The note forgets each one only once it is gone.
    #[test]
    fn a_wake_ends_the_orphan_holding_its_conversation_and_nothing_else() {
        let root = tempfile::tempdir().expect("a data root");
        let (person, mut with_terminal) = orphan_at(60955, Some(Signal::Terminate));
        with_terminal.seen.terminal = true;
        write_left(
            root.path(),
            &[
                group(4, 60693, "coordinator"),
                group(7, 60171, "work"),
                group(9, 60955, "person"),
            ],
        )
        .expect("the way out's note");
        let table = Table::of(&[
            orphan_at(60693, None),
            orphan_at(60171, Some(Signal::Terminate)),
            (person, with_terminal),
        ]);
        let book = Leftovers::new();
        let lines = RefCell::new(Vec::new());
        let note = |line: &str| lines.borrow_mut().push(line.to_string());
        book.end_proven(
            root.path(),
            &table,
            |held| held.agent == "zo" && held.session.as_deref() == Some("coordinator"),
            &note,
        );
        assert_eq!(
            read_note(root.path()),
            [group(7, 60171, "work"), group(9, 60955, "person")],
            "the note forgets the ended orphan and keeps the rest"
        );
        assert_eq!(
            table.signals_to(60693),
            vec![Signal::Terminate, Signal::Kill]
        );
        assert!(!table.alive(60693));
        assert!(
            table.signals_to(60171).is_empty(),
            "another conversation's orphan was ended"
        );
        assert_eq!(
            lines.borrow().as_slice(),
            [
                "boot: ended the last window's pane agent left without a terminal — pid 60693 \
              (terminal 4, zo, session coordinator)"
            ]
        );
        book.end_proven(root.path(), &table, |_| true, &note);
        assert_eq!(table.signals_to(60171), vec![Signal::Terminate]);
        assert!(
            table.signals_to(60955).is_empty(),
            "a zo with a terminal was ended"
        );
        assert!(table.alive(60955));
        assert_eq!(
            table.signals_to(60693),
            vec![Signal::Terminate, Signal::Kill],
            "an orphan was ended twice"
        );
        assert_eq!(lines.borrow().len(), 2, "{:#?}", lines.borrow());
        assert_eq!(
            read_note(root.path()),
            [group(9, 60955, "person")],
            "a process never proven is not forgotten"
        );
    }

    fn read_note(root: &Path) -> Vec<PaneGroup> {
        std::fs::read(root.join(LEFT_FILE))
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    /// t-76504 F5: the boot ends the last window's leftovers and the window
    /// dies part-way — at any wait of the ending, the note on the disk still
    /// names every group not yet seen gone, so the next boot reads them all.
    /// A group seen gone is forgotten; one the KILL did not end, and one
    /// never proven, stay written down; and the next boot, finding the
    /// stubborn one ended by then, forgets it.
    #[test]
    fn the_note_keeps_every_group_until_it_is_seen_gone() {
        let root = tempfile::tempdir().expect("a data root");
        let (stubborn, mut held) = orphan_at(60693, None);
        held.unkillable = true;
        let (person, mut with_terminal) = orphan_at(60955, None);
        with_terminal.seen.terminal = true;
        let written = [
            group(4, stubborn, "coordinator"),
            group(7, 60171, "gone"),
            group(9, person, "person"),
        ];
        write_left(root.path(), &written).expect("the way out's note");
        let mut table = Table::of(&[(stubborn, held), (person, with_terminal)]);
        table.note_at = Some(root.path().to_path_buf());
        let lines = RefCell::new(Vec::new());
        let note = |line: &str| lines.borrow_mut().push(line.to_string());
        Leftovers::new().end_proven(root.path(), &table, |_| true, &note);
        assert!(!table.notes.borrow().is_empty(), "the ending never waited");
        for (at, seen) in table.notes.borrow().iter().enumerate() {
            for one in [&written[0], &written[2]] {
                assert!(seen.contains(one), "wait {at}: the note lost {one:?}");
            }
        }
        assert_eq!(
            table.signals_to(stubborn),
            vec![Signal::Terminate, Signal::Kill]
        );
        assert_eq!(
            read_note(root.path()),
            [written[0].clone(), written[2].clone()],
            "the gone one is forgotten, the rest kept"
        );
        assert_eq!(
            lines.borrow().as_slice(),
            [
                "boot: the last window's pane agent did not end on TERM or KILL — pid 60693 \
                 (terminal 4, zo, session coordinator)"
            ]
        );

        // The next boot: the stubborn one has gone since.
        table.procs.borrow_mut().remove(&stubborn);
        let next = Leftovers::new();
        next.end_proven(root.path(), &table, |_| true, &note);
        assert_eq!(read_note(root.path()), [written[2].clone()]);
        assert_eq!(
            table.signals_to(stubborn).len(),
            2,
            "a gone pid was signalled"
        );
    }

    /// The way out writes its own groups beside what the last window left
    /// and this one has not seen gone — never over it.
    #[test]
    fn the_way_out_writes_beside_the_last_windows_note() {
        let root = tempfile::tempdir().expect("a data root");
        let last = group(9, 60955, "person");
        write_left(root.path(), std::slice::from_ref(&last)).expect("the last window's note");
        let book = Leftovers::new();
        let own = group(3, 71000, "mine");
        book.keep_own(root.path(), std::slice::from_ref(&own))
            .expect("written");
        assert_eq!(read_note(root.path()), [own, last.clone()]);
        book.keep_own(root.path(), &[]).expect("written");
        assert_eq!(read_note(root.path()), [last]);
    }

    /// t-76504 F3: the way out's collection stands on a pane's lock another
    /// thread holds and never lets go. The main thread waits for the note no
    /// longer than its patience, and is told nothing was written.
    #[test]
    fn the_way_out_waits_no_longer_than_its_patience_for_a_stuck_lock() {
        static BOOK: Leftovers = Leftovers::new();
        static PANE: Mutex<()> = Mutex::new(());
        let root = tempfile::tempdir().expect("a data root");
        let held = PANE.lock().expect("the stuck thread's hold");
        let (done, finished) = std::sync::mpsc::channel();
        let at = root.path().to_path_buf();
        let leaving = std::thread::spawn(move || {
            let written = leave_within(
                &BOOK,
                at,
                || {
                    let _pane = PANE.lock();
                    Vec::new()
                },
                || Table::of(&[]),
                |_, _| {},
                Duration::from_millis(10),
            );
            let _ = done.send(written);
        });
        let answer = finished.recv_timeout(Duration::from_secs(5));
        drop(held);
        leaving.join().expect("the way out");
        assert_eq!(
            answer,
            Ok(false),
            "the way out stood on a lock nobody let go"
        );
        // The main thread's step hands the collection to that road.
        let main = include_str!("main.rs");
        let step = main
            .split("fn hang_up_panes(&self) {")
            .nth(1)
            .and_then(|rest| rest.split("\n    }\n").next())
            .expect("hang_up_panes");
        let moved = step.find("move ||").expect("the collection is moved off");
        let collected = step.find("pane_groups(").expect("pane_groups");
        assert!(moved < collected, "{step}");
    }

    /// The note the way out leaves reads back as written, and nothing to
    /// write removes an older one.
    #[test]
    fn the_note_reads_back_and_nothing_left_removes_it() {
        let root = tempfile::tempdir().expect("a data root");
        let groups = vec![group(4, 60693, "coordinator")];
        write_left(root.path(), &groups).expect("written");
        assert_eq!(read_note(root.path()), groups);
        write_left(root.path(), &[]).expect("removed");
        assert!(!root.path().join(LEFT_FILE).exists());
    }

    /// How long a terminal may take to hang up after its last master
    /// closed: spent in full only by a failure.
    #[cfg(target_os = "macos")]
    const HANGUP_PATIENCE: Duration = Duration::from_secs(10);
    /// The pause between two looks at a terminal that has not hung up.
    #[cfg(target_os = "macos")]
    const HANGUP_PROBE_PAUSE: Duration = Duration::from_millis(10);

    /// One `openpty`, the descriptors it opened or the error it set.
    #[cfg(target_os = "macos")]
    fn raw_openpty() -> std::io::Result<(libc::c_int, libc::c_int)> {
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: `openpty` writes the two descriptors; no name, settings
        // or size is asked for.
        let opened = unsafe {
            libc::openpty(
                &raw mut master,
                &raw mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if opened == 0 {
            // `openpty` leaves both ends open across an exec, as `portable_pty`
            // does not: a child another test starts in between would inherit
            // the master and hold the terminal open past its close.
            for fd in [master, slave] {
                // SAFETY: `fcntl` sets a flag on a descriptor opened above.
                unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
            }
            Ok((master, slave))
        } else {
            Err(std::io::Error::last_os_error())
        }
    }

    /// A pty pair from `open`, as the test opens one: asked again where a
    /// concurrent open took the pty (`zerocode_pty::ask_for_pty` owns which
    /// errors and how often).
    #[cfg(target_os = "macos")]
    fn open_pair(
        open: impl FnMut() -> std::io::Result<(libc::c_int, libc::c_int)>,
    ) -> std::io::Result<(libc::c_int, libc::c_int)> {
        zerocode_pty::ask_for_pty(open, std::io::Error::raw_os_error)
    }

    /// The test's pty open survives a concurrent open taking the pty it
    /// was about to get (t-77996 review P3-5): two `-6`s from the kernel,
    /// then a pair. Counted asks, no clock, no load.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_tests_pty_open_survives_a_lost_race() {
        let mut asked = 0;
        let pair = open_pair(|| {
            asked += 1;
            if asked <= 2 {
                return Err(std::io::Error::from_raw_os_error(-6));
            }
            Ok((7, 8))
        });
        assert_eq!(pair.expect("the pty never opened"), (7, 8));
        assert_eq!(asked, 3);
    }

    /// Whether `probe` answers yes before [`HANGUP_PATIENCE`] runs out.
    /// A hangup that is already there passes at once; the patience is spent
    /// only when it is not, and the last answer is the one returned.
    #[cfg(target_os = "macos")]
    fn hangs_up_within(mut probe: impl FnMut() -> bool) -> bool {
        let end = std::time::Instant::now() + HANGUP_PATIENCE;
        loop {
            if probe() {
                return true;
            }
            if std::time::Instant::now() >= end {
                return false;
            }
            std::thread::sleep(HANGUP_PROBE_PAUSE);
        }
    }

    /// A pty the test opens passes nothing to a child it never meant to
    /// feed: both ends close on exec, as `portable_pty` sets them.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_tests_pty_closes_on_exec() {
        let (master, slave) = open_pair(raw_openpty).expect("openpty");
        let flags = |fd| {
            // SAFETY: `fcntl` reads the flags of a descriptor opened above.
            unsafe { libc::fcntl(fd, libc::F_GETFD) }
        };
        let (master_flags, slave_flags) = (flags(master), flags(slave));
        // SAFETY: both opened above, closed once.
        unsafe {
            libc::close(master);
            libc::close(slave);
        }
        assert!(master_flags & libc::FD_CLOEXEC != 0, "the master leaks");
        assert!(slave_flags & libc::FD_CLOEXEC != 0, "the slave leaks");
    }

    /// The held-back hangup's own process: see
    /// [`a_hangup_held_back_by_a_copy_of_the_master_is_waited_for`].
    #[cfg(target_os = "macos")]
    const HELD_BACK_ALONE: &str =
        "pane_leftovers::tests::a_hangup_held_back_by_a_copy_of_the_master_alone";

    /// A copy of the master held by something else (a child a parallel
    /// test started in between) delays the hangup; the wait covers it. The
    /// copy is dropped by the third probe, with no clock to race — in a
    /// process of its own ([`HELD_BACK_ALONE`]), where no other test starts
    /// a child. Beside them, a child started between `openpty` and the
    /// close-on-exec flags [`raw_openpty`] sets after it kept the master for
    /// its whole life, and the hangup came a probe late (2/30 beside
    /// `artifact_runtime`, t-84451).
    #[cfg(target_os = "macos")]
    #[test]
    fn a_hangup_held_back_by_a_copy_of_the_master_is_waited_for() {
        let alone = crate::proc::quiet_command(std::env::current_exe().expect("this test binary"))
            .args(["--exact", HELD_BACK_ALONE, "--ignored", "--test-threads=1"])
            .output()
            .expect("the held-back hangup ran in a process of its own");
        let said = String::from_utf8_lossy(&alone.stdout);
        assert!(
            alone.status.success() && said.contains("test result: ok. 1 passed"),
            "the held-back hangup failed in its own process: {said}{}",
            String::from_utf8_lossy(&alone.stderr)
        );
    }

    /// The held-back hangup itself, started by
    /// [`a_hangup_held_back_by_a_copy_of_the_master_is_waited_for`]; never
    /// part of a normal run.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore = "the held-back hangup's own process, started by the test that waits for it"]
    fn a_hangup_held_back_by_a_copy_of_the_master_alone() {
        let (master, slave) = open_pair(raw_openpty).expect("openpty");
        // SAFETY: duplicates the master opened above. The copy closes on
        // exec like the master: `dup` drops that, and a child a parallel test
        // started then held a copy this test never closes (t-84259).
        let copy = unsafe { libc::fcntl(master, libc::F_DUPFD_CLOEXEC, 0) };
        assert!(copy >= 0, "dup: {}", std::io::Error::last_os_error());
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `fstat` fills the struct for an open descriptor.
        assert_eq!(unsafe { libc::fstat(slave, stat.as_mut_ptr()) }, 0);
        // SAFETY: filled above.
        let device = unsafe { stat.assume_init() }.st_rdev;
        let device = u32::from_ne_bytes(device.to_ne_bytes());
        // SAFETY: the master opened above, closed once.
        unsafe { libc::close(master) };

        let mut probes = 0;
        let answer = hangs_up_within(|| {
            probes += 1;
            if probes == 3 {
                // SAFETY: the copy duplicated above, closed once.
                unsafe { libc::close(copy) };
            }
            hung_up(device)
        });
        // SAFETY: the slave opened above, closed once.
        unsafe { libc::close(slave) };
        assert!(
            answer,
            "a terminal whose last master closed did not hang up"
        );
        assert_eq!(probes, 3);
    }

    /// A terminal hangs up when its master closes, and not before
    /// (t-77414): a leftover that ignored the window's hangup still holds
    /// its terminal, and this is what tells that terminal from a live one.
    /// A real pty, no process signalled.
    #[cfg(target_os = "macos")]
    #[test]
    fn a_terminal_hangs_up_when_its_master_closes() {
        let (master, slave) = open_pair(raw_openpty).expect("openpty");
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `fstat` fills the struct for an open descriptor.
        assert_eq!(unsafe { libc::fstat(slave, stat.as_mut_ptr()) }, 0);
        // SAFETY: filled above.
        let device = unsafe { stat.assume_init() }.st_rdev;
        let device = u32::from_ne_bytes(device.to_ne_bytes());

        assert!(!hung_up(device), "a terminal whose master is open hung up");
        // SAFETY: the master opened above, closed once; the slave stays
        // open, as a leftover's descriptors keep its terminal.
        unsafe { libc::close(master) };
        let answer = hangs_up_within(|| hung_up(device));
        // SAFETY: the slave opened above, closed once.
        unsafe { libc::close(slave) };
        assert!(answer, "a terminal whose master closed did not hang up");
        assert!(!hung_up(u32::MAX), "no device is not a hung-up terminal");
    }
}
