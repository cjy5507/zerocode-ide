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
//! So the way out sends every agent pane's process group a hangup, a short
//! grace later a TERM, and at its deadline a KILL ([`end`]), on a thread of
//! its own — the way out's own deadlines are not lengthened and its main
//! thread waits on none of it — after writing each group down
//! ([`LEFT_FILE`]). A restart replaces the process within milliseconds, so
//! that thread may not see its deadline; whatever it did not see end, the
//! next window finds in the note. There a process is the last window's only
//! when all four hold: the note names it, it began at the instant the note
//! says (no later process that took its pid), its parent is launchd, and it
//! has no terminal ([`proven`]). Each one proven is sent a TERM, then a KILL
//! at the deadline ([`BOOT_PLAN`]) — by the boot's sweep, or first by the
//! wake that reopens its conversation ([`free_conversation`]), so the
//! resume no longer meets its lease. A zo somebody opened in a terminal, or
//! one holding another conversation, is never named by the note and never
//! proven.
//!
//! Process groups and signals are Unix's; on Windows no group is collected
//! and nothing is signalled, and the pane's ConPTY closing is all that ends
//! its agent there, as before.

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
    /// The group's leader, whose pid is the group's id: the pane's own child
    /// (it leads the pane's session), or the job a shell put in front.
    pub(crate) leader: u32,
    pub(crate) agent: String,
    /// The conversation the pane reported, when it reported one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) session: Option<String>,
    /// When the leader began, in microseconds since the epoch — what tells
    /// it from a later process that took its pid. `None` where the host
    /// could not say: such a group is never proven at the next boot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) started_us: Option<u64>,
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
}

/// What the host says of one process right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Seen {
    pub(crate) parent: u32,
    /// Whether it has a controlling terminal.
    pub(crate) terminal: bool,
    pub(crate) started_us: u64,
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
    fn seen(&self, pid: u32) -> Option<Seen>;
    /// Send `signal` to the process group `group`.
    fn signal_group(&self, group: u32, signal: Signal);
    /// Whether any process of `group` is left.
    fn group_alive(&self, group: u32) -> bool;
    /// Time since an origin of the host's own.
    fn elapsed(&self) -> Duration;
    fn wait(&self, span: Duration);
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

/// End `groups` by `plan`, and answer the ones left. A group whose leader
/// is a later process than the one written down is not touched.
pub(crate) fn end(groups: Vec<PaneGroup>, host: &dyn Host, plan: Plan) -> Vec<PaneGroup> {
    let mut live: Vec<PaneGroup> = groups
        .into_iter()
        .filter(|group| host.group_alive(group.leader) && same_leader(group, host))
        .collect();
    let from = host.elapsed();
    if plan.hangup {
        send(&live, host, Signal::Hangup);
        wait_out(&mut live, host, from + plan.grace, plan.poll);
    }
    send(&live, host, Signal::Terminate);
    wait_out(&mut live, host, from + plan.deadline, plan.poll);
    send(&live, host, Signal::Kill);
    let killed = host.elapsed();
    wait_out(&mut live, host, killed + plan.after_kill, plan.poll);
    live
}

/// Whether the group's leader is still the process written down: the same
/// beginning, or a leader gone while its group stands (a group's id is not
/// handed to another process while any member is left).
fn same_leader(group: &PaneGroup, host: &dyn Host) -> bool {
    match (group.started_us, host.seen(group.leader)) {
        (Some(started), Some(seen)) => seen.started_us == started,
        _ => true,
    }
}

fn send(live: &[PaneGroup], host: &dyn Host, signal: Signal) {
    for group in live {
        host.signal_group(group.leader, signal);
    }
}

/// Look until every group in `live` is gone or `until` comes.
fn wait_out(live: &mut Vec<PaneGroup>, host: &dyn Host, until: Duration, poll: Duration) {
    loop {
        live.retain(|group| host.group_alive(group.leader));
        let now = host.elapsed();
        if live.is_empty() || now >= until {
            return;
        }
        host.wait(poll.min(until - now));
    }
}

/// Whether `group`, written down by the last window's way out, is that
/// window's leftover: still the process it was (the same beginning), handed
/// to launchd, and holding no terminal. A group whose beginning was never
/// read is never proven.
pub(crate) fn proven(group: &PaneGroup, host: &dyn Host) -> bool {
    let Some(started) = group.started_us else {
        return false;
    };
    host.seen(group.leader)
        .is_some_and(|seen| seen.started_us == started && seen.parent == LAUNCHD && !seen.terminal)
}

/// The log line for a group the way out could not end.
pub(crate) fn left_line(group: &PaneGroup) -> String {
    format!(
        "exit: a pane's agent did not end within {} ms — {}",
        EXIT_PLAN.deadline.as_millis() + EXIT_PLAN.after_kill.as_millis(),
        group.named()
    )
}

/// Write `groups` down for the next window. Nothing to write removes any
/// older note.
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

/// The way out's step: write `groups` down, then end them on a thread of
/// its own by [`EXIT_PLAN`], naming in the log each one left, and writing
/// down only those for the next window — if this process lives that long.
pub(crate) fn leave(root: PathBuf, groups: Vec<PaneGroup>, note: fn(&Path, &str)) {
    if groups.is_empty() {
        let _ = write_left(&root, &[]);
        return;
    }
    if let Err(error) = write_left(&root, &groups) {
        note(
            &root,
            &format!("exit: the pane agents were not written down: {error}"),
        );
    }
    let spawned = std::thread::Builder::new()
        .name("zerocode-pane-hangup".to_string())
        .spawn(move || {
            let left = end(groups, &System::new(), EXIT_PLAN);
            for group in &left {
                note(&root, &left_line(group));
            }
            let _ = write_left(&root, &left);
        });
    if let Err(error) = spawned {
        eprintln!("zerocode-shell: the pane hangup thread did not start: {error}");
    }
}

/// What the last window wrote down, as this process holds it: each group
/// waiting, being ended, or done with.
#[derive(Debug, Default)]
struct Book {
    read: bool,
    groups: Vec<(PaneGroup, Phase)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Waiting,
    Ending,
    Done,
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
            }),
            done: Condvar::new(),
        }
    }

    /// Read the last window's note off the disk, once per process, and
    /// remove it: whatever this window ends, it ends from its own book.
    fn read_once(&self, root: &Path) {
        let mut book = self.book.lock().unwrap_or_else(PoisonError::into_inner);
        if std::mem::replace(&mut book.read, true) {
            return;
        }
        let path = root.join(LEFT_FILE);
        let groups: Vec<PaneGroup> = crate::durable_file::read_plain_file(&path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default();
        let _ = crate::durable_file::remove_file(&path);
        book.groups = groups
            .into_iter()
            .map(|group| (group, Phase::Waiting))
            .collect();
    }

    /// Take the waiting groups `wanted` picks for ending, and wait out any
    /// it picks that another road is ending now.
    fn take(&self, wanted: impl Fn(&PaneGroup) -> bool) -> Vec<PaneGroup> {
        let mut book = self.book.lock().unwrap_or_else(PoisonError::into_inner);
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

    fn finish(&self, ended: &[PaneGroup]) {
        let mut book = self.book.lock().unwrap_or_else(PoisonError::into_inner);
        for (group, phase) in &mut book.groups {
            if ended.contains(group) {
                *phase = Phase::Done;
            }
        }
        drop(book);
        self.done.notify_all();
    }

    /// End every group `wanted` picks that [`proven`] holds for, by
    /// [`BOOT_PLAN`], and say each one in the log: ended, or left.
    fn end_proven(
        &self,
        root: &Path,
        host: &dyn Host,
        wanted: impl Fn(&PaneGroup) -> bool,
        note: &dyn Fn(&str),
    ) {
        self.read_once(root);
        let taken = self.take(wanted);
        if taken.is_empty() {
            return;
        }
        let ours: Vec<PaneGroup> = taken
            .iter()
            .filter(|group| proven(group, host))
            .cloned()
            .collect();
        let left = end(ours.clone(), host, BOOT_PLAN);
        for group in &ours {
            note(&if left.contains(group) {
                format!(
                    "boot: the last window's pane agent did not end on TERM or KILL — {}",
                    group.named()
                )
            } else {
                format!(
                    "boot: ended the last window's pane agent left without a terminal — {}",
                    group.named()
                )
            });
        }
        self.finish(&taken);
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

/// The window's own pane groups, read for the way out: for every pane this
/// window knows an agent for, its child's group and the group in front of
/// its terminal when that is another (an agent typed into a shell).
pub(crate) fn pane_groups(state: &crate::AppState) -> Vec<PaneGroup> {
    use crate::ShellStateExt as _;
    use zerocode_lane::PtyTransport as _;
    // No process groups to signal on Windows: nothing is collected there.
    if cfg!(not(unix)) {
        return Vec::new();
    }
    let agents = state.agent_terms().clone();
    let sessions = state.pane_sessions().clone();
    let mut groups = Vec::new();
    for (term, agent) in agents {
        let Some(held) = state.terminals().handle(term) else {
            continue;
        };
        let (child, front) = {
            let pty = crate::terminal_registry::lock_pty(&held);
            (pty.pid(), pty.foreground_process_id())
        };
        let mut leaders: Vec<u32> = child.into_iter().chain(front).collect();
        leaders.dedup();
        for leader in leaders {
            groups.push(PaneGroup {
                term,
                leader,
                agent: agent.to_string(),
                session: sessions.get(&term).map(|session| session.id.clone()),
                started_us: System::new().seen(leader).map(|seen| seen.started_us),
            });
        }
    }
    groups
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
    fn seen(&self, pid: u32) -> Option<Seen> {
        seen_by_the_kernel(pid)
    }

    fn signal_group(&self, group: u32, signal: Signal) {
        signal_group(group, signal);
    }

    fn group_alive(&self, group: u32) -> bool {
        group_alive(group)
    }

    fn elapsed(&self) -> Duration {
        self.origin.elapsed()
    }

    fn wait(&self, span: Duration) {
        std::thread::sleep(span);
    }
}

/// One process as macOS's `proc_pidinfo` says it: parent, controlling
/// terminal, and beginning, in one call and no child process.
#[cfg(target_os = "macos")]
fn seen_by_the_kernel(pid: u32) -> Option<Seen> {
    let pid = libc::c_int::try_from(pid).ok().filter(|pid| *pid > 0)?;
    let mut info = std::mem::MaybeUninit::<libc::proc_bsdinfo>::uninit();
    let size = libc::c_int::try_from(std::mem::size_of::<libc::proc_bsdinfo>()).ok()?;
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
        return None;
    }
    // SAFETY: the call above filled the whole struct.
    let info = unsafe { info.assume_init() };
    Some(Seen {
        parent: info.pbi_ppid,
        // `e_tdev` is NODEV (all bits set) for a process with no terminal.
        terminal: info.e_tdev != u32::MAX && info.e_tdev != 0,
        started_us: info
            .pbi_start_tvsec
            .saturating_mul(1_000_000)
            .saturating_add(info.pbi_start_tvusec),
    })
}

/// Elsewhere the window reads no beginning, so nothing is ever proven.
#[cfg(not(target_os = "macos"))]
fn seen_by_the_kernel(_pid: u32) -> Option<Seen> {
    None
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

#[cfg(unix)]
fn group_alive(group: u32) -> bool {
    group > LAUNCHD && crate::codex_queue::process_group_exists(group)
}

#[cfg(not(unix))]
fn group_alive(_group: u32) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::collections::HashMap;

    /// A fake process: what the table says of it, and what it does with each
    /// signal — `ends_on` the first signal it leaves on, `None` for one that
    /// nothing but a KILL ends (and not even that when `unkillable`).
    #[derive(Clone)]
    struct Fake {
        seen: Seen,
        ends_on: Option<Signal>,
        unkillable: bool,
        alive: bool,
    }

    /// A process table and a clock of its own: `wait` moves the clock, and
    /// every signal is written down in order.
    struct Table {
        procs: RefCell<HashMap<u32, Fake>>,
        now: RefCell<Duration>,
        sent: RefCell<Vec<(u32, Signal, Duration)>>,
    }

    impl Table {
        fn of(procs: &[(u32, Fake)]) -> Self {
            Self {
                procs: RefCell::new(procs.iter().cloned().collect()),
                now: RefCell::new(Duration::ZERO),
                sent: RefCell::new(Vec::new()),
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
    }

    impl Host for Table {
        fn seen(&self, pid: u32) -> Option<Seen> {
            self.procs
                .borrow()
                .get(&pid)
                .filter(|one| one.alive)
                .map(|one| one.seen)
        }

        fn signal_group(&self, group: u32, signal: Signal) {
            self.sent
                .borrow_mut()
                .push((group, signal, *self.now.borrow()));
            if let Some(one) = self.procs.borrow_mut().get_mut(&group) {
                let ends = match signal {
                    Signal::Kill => !one.unkillable,
                    _ => one.ends_on == Some(signal),
                };
                if ends {
                    one.alive = false;
                }
            }
        }

        fn group_alive(&self, group: u32) -> bool {
            self.alive(group)
        }

        fn elapsed(&self) -> Duration {
            *self.now.borrow()
        }

        fn wait(&self, span: Duration) {
            *self.now.borrow_mut() += span;
        }
    }

    const BORN: u64 = 1_793_000_000_000_000;

    fn orphan(ends_on: Option<Signal>) -> Fake {
        Fake {
            seen: Seen {
                parent: LAUNCHD,
                terminal: false,
                started_us: BORN,
            },
            ends_on,
            unkillable: false,
            alive: true,
        }
    }

    fn group(term: u32, leader: u32, session: &str) -> PaneGroup {
        PaneGroup {
            term,
            leader,
            agent: "zo".to_string(),
            session: Some(session.to_string()),
            started_us: Some(BORN),
        }
    }

    /// The way out ends every pane's group in its deadline: one that leaves
    /// on the hangup gets nothing more, one that ignores the hangup gets a
    /// TERM at the grace, one that ignores both gets a KILL at the deadline —
    /// and none of it outlasts the plan.
    #[test]
    fn the_way_out_ends_a_child_that_ignores_the_hangup_within_its_deadline() {
        let table = Table::of(&[
            (60171, orphan(Some(Signal::Hangup))),
            (60693, orphan(Some(Signal::Terminate))),
            (60955, orphan(None)),
        ]);
        let left = end(
            vec![
                group(4, 60171, "s-1"),
                group(7, 60693, "s-2"),
                group(9, 60955, "s-3"),
            ],
            &table,
            EXIT_PLAN,
        );
        assert!(left.is_empty(), "{left:?}");
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
        let mut stuck = orphan(None);
        stuck.unkillable = true;
        let table = Table::of(&[(60955, stuck)]);
        let left = end(vec![group(9, 60955, "s-3")], &table, EXIT_PLAN);
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
        let mut later = orphan(None);
        later.seen.started_us = BORN + 1;
        let table = Table::of(&[(60171, later)]);
        assert!(end(vec![group(4, 60171, "s-1")], &table, EXIT_PLAN).is_empty());
        assert!(table.sent.borrow().is_empty());
        assert!(table.alive(60171));
    }

    /// The next boot's proof: the note names it, the same beginning, parent
    /// launchd, no terminal. A zo a person opened in a terminal, one whose
    /// parent is alive, one whose pid a later process took, and one whose
    /// beginning was never read are never proven.
    #[test]
    fn only_the_last_windows_own_orphan_is_proven() {
        let mut with_terminal = orphan(None);
        with_terminal.seen.terminal = true;
        let mut parented = orphan(None);
        parented.seen.parent = 4242;
        let mut later = orphan(None);
        later.seen.started_us = BORN + 7;
        let table = Table::of(&[
            (60171, orphan(None)),
            (70001, with_terminal),
            (70002, parented),
            (70003, later),
        ]);
        assert!(proven(&group(4, 60171, "s-1"), &table));
        for pid in [70001, 70002, 70003] {
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
        assert!(
            !proven(&group(4, 99999, "s-1"), &table),
            "a gone pid was proven"
        );
    }

    /// t-76434, the next boot: the last window wrote down three zo panes. A
    /// wake reopening one conversation ends that conversation's proven
    /// orphan — TERM, and KILL at the deadline when TERM is not enough — and
    /// touches neither another conversation's orphan nor a process that is
    /// not proven; the boot's sweep then ends the rest that are proven, and
    /// nothing twice.
    #[test]
    fn a_wake_ends_the_orphan_holding_its_conversation_and_nothing_else() {
        let root = tempfile::tempdir().expect("a data root");
        let mut with_terminal = orphan(Some(Signal::Terminate));
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
            (60693, orphan(None)),
            (60171, orphan(Some(Signal::Terminate))),
            (60955, with_terminal),
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
        assert!(
            !root.path().join(LEFT_FILE).exists(),
            "the note is read once and removed"
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
    }

    /// The note the way out leaves reads back as written, and nothing to
    /// write removes an older one.
    #[test]
    fn the_note_reads_back_and_nothing_left_removes_it() {
        let root = tempfile::tempdir().expect("a data root");
        let groups = vec![group(4, 60693, "coordinator")];
        write_left(root.path(), &groups).expect("written");
        let read: Vec<PaneGroup> =
            serde_json::from_slice(&std::fs::read(root.path().join(LEFT_FILE)).expect("bytes"))
                .expect("json");
        assert_eq!(read, groups);
        write_left(root.path(), &[]).expect("removed");
        assert!(!root.path().join(LEFT_FILE).exists());
    }
}
