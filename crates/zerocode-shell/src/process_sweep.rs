//! The window's half of `process-sweep` (t-78914): the process table, where
//! each listed process stands, the protection judgment and the one signal.
//!
//! The ledger decided the mode, the needle or the pids and the run
//! (`zerocode_core::orchestration::process_sweep`); this reads the system and
//! carries it out. The table is the status bar's own read
//! ([`crate::resource_usage::enumerate_processes`]), read once per call, and
//! the places are the port list's ([`crate::system_runtime::process_cwds`]).
//! The judgment is the kill guard's (t-80602, [`kill::judge_kill`]) over that
//! same read and the guard's own reading of the window's panes
//! ([`crate::kill_witness`]), so a pane's `kill` and this verb are judged by
//! one rule.

use std::collections::HashMap;

use zerocode_core::orchestration::process_sweep::{
    self as sweep, Checkout, Judge, Judgment, ProcessRow, ProcessSweepAsk, Signals,
};
use zerocode_core::shell_rule::kill::{self, KillCall, KillTarget, KillVerdict, ProcessTable};
use zerocode_hookd::KillRoots;

/// What one call carried: the verb's answer, and for an end call the line the
/// ledger keeps of it.
pub(crate) struct Carried {
    pub(crate) said: String,
    pub(crate) filed: Option<Filed>,
}

/// One end call's line, for its run's coordinator, filed once under `receipt`.
pub(crate) struct Filed {
    pub(crate) run: String,
    pub(crate) line: String,
    pub(crate) receipt: String,
}

/// Carry one decided call against this machine. `roots` is the asking pane's
/// own process and the trees it may not signal, as the window reads them;
/// `coordinator_trees` are where the ledger's live coordinator seats stand
/// ([`coordinator_trees`]); `team` and `pane` are the seat that asked, by which
/// the processes above the asking call are found ([`asking_pids`]).
pub(crate) fn carry(
    ask: &ProcessSweepAsk,
    roots: Option<KillRoots>,
    coordinator_trees: &[String],
    team: &str,
    pane: &str,
) -> Result<Carried, String> {
    let (rows, table) = table()?;
    let checkouts = held_checkouts(ask, coordinator_trees);
    let asking = asking_pids(&rows, team, pane);
    let judge = judge_with(
        roots,
        table,
        checkouts.clone(),
        std::process::id(),
        asking,
        Box::new(crate::stray_reap::named_by_coordinator),
    );
    carry_with(
        ask,
        &checkouts,
        rows,
        &crate::system_runtime::process_cwds,
        judge.as_ref(),
        &System::new(),
    )
}

/// The pids of the call that is asking and of every process above it, read from the table the
/// call is judged over (t-84800 r2 P2-2, r1-2).
///
/// A call from a pane reaches the window through the shim's `curl`, and that `curl` carries the
/// team and the pane in its own arguments (the headers the window reads them from). So the
/// asking call is the `curl` row naming this team and pane, and everything above it — the shim's
/// shell, the shell a coordinator's tool started, the agent, the pane's shell, the window — is
/// found by the parent each row names, not by what any of them spells. A pane that has several
/// calls in flight has all of them found: more is hidden and refused, never less. Empty when the
/// table holds no such call (a shim that sends its headers some other way) or the seat is not
/// named.
fn asking_pids(rows: &[ProcessRow], team: &str, pane: &str) -> Vec<u32> {
    if team.is_empty() || pane.is_empty() {
        return Vec::new();
    }
    let parents: HashMap<u32, u32> = rows.iter().map(|row| (row.pid, row.ppid)).collect();
    let mut asking: Vec<u32> = Vec::new();
    for call in rows.iter().filter(|row| is_call_of(&row.args, team, pane)) {
        let mut at = call.pid;
        while at > 1 && !asking.contains(&at) {
            asking.push(at);
            match parents.get(&at) {
                Some(parent) => at = *parent,
                None => break,
            }
        }
    }
    asking.sort_unstable();
    asking
}

/// Whether a command line is the shim's request to the window's team road for this seat: its
/// address ends in `/agent-teams`, and the team and pane headers are followed by this team and
/// pane as words of their own.
fn is_call_of(args: &str, team: &str, pane: &str) -> bool {
    let words: Vec<&str> = args.split_whitespace().collect();
    let header = |name: &str, value: &str| {
        let key = format!("{name}:");
        words
            .windows(2)
            .any(|pair| pair[0] == key && pair[1] == value)
    };
    words.iter().any(|word| word.ends_with("/agent-teams"))
        && header(zerocode_hookd::TEAM_ID_HEADER, team)
        && header(zerocode_hookd::TEAM_PANE_HEADER, pane)
}

/// The worker checkouts this call reads: the ask's, less each that is or
/// holds a coordinator's tree or a run's project root.
///
/// Paths are read as the system names them (r1-10): `lsof` prints a process's real folder, so a
/// checkout or a tree told through a link is compared by where the link leads. A checkout whose
/// folder was made again after its worker began is nobody's (r3w-6), and so is one that is now a
/// link: `git worktree add` never makes one, and a link leads wherever it was pointed — to an older
/// folder that passes the check above, even to the main checkout. The main checkout is taken out
/// again after the paths are read this way, since the ledger could only compare them as told
/// (t-84800 r2 P2-3).
fn held_checkouts(ask: &ProcessSweepAsk, coordinator_trees: &[String]) -> Vec<Checkout> {
    let mut trees: Vec<String> = coordinator_trees.iter().map(|tree| real(tree)).collect();
    for root in ask.project_roots() {
        trees.push(root.clone());
        trees.push(real(root));
    }
    let told: Vec<Checkout> = ask
        .checkouts()
        .iter()
        .filter(|checkout| !is_link(&checkout.path) && !made_again(checkout))
        .map(|checkout| Checkout {
            path: real(&checkout.path),
            ..checkout.clone()
        })
        .collect();
    sweep::outside(told, &trees)
}

/// Whether the last part of `path` is a link. A path the system cannot read is not one.
fn is_link(path: &str) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|named| named.file_type().is_symlink())
}

/// `path` as the file system spells it, links resolved; as told when it cannot be read (a folder
/// gone, a path the system does not know), which compares as before.
fn real(path: &str) -> String {
    std::fs::canonicalize(path).map_or_else(
        |_| path.to_string(),
        |real| real.to_string_lossy().into_owned(),
    )
}

/// Whether the folder now at the checkout's path was born after the worker that reported it began.
/// A birth the system does not say counts as not made again, which is the reading before this.
fn made_again(checkout: &Checkout) -> bool {
    let born = std::fs::metadata(&checkout.path)
        .and_then(|held| held.created())
        .ok()
        .and_then(|born| born.duration_since(std::time::UNIX_EPOCH).ok())
        .and_then(|since| i64::try_from(since.as_millis()).ok());
    born.is_some_and(|born| sweep::recreated(born, checkout.started_ms))
}

/// The tree each live coordinator seat (`team/pane`) stands in, as the window
/// recorded it when it placed that pane. A coordinator's tree is nobody's
/// worker checkout, whatever a worker row reported (r2 P1-1): a worker opened
/// without a tree of its own reports the coordinator's. A seat this window
/// holds no pane or no tree for adds nothing — the ledger has already left
/// out every row that was not a tree of the worker's own.
pub(crate) fn coordinator_trees(
    seats: &[String],
    host: &dyn crate::agent_teams::Host,
) -> Vec<String> {
    // The terms first, the table let go before the host is asked.
    let terms: Vec<u32> = {
        let teams = crate::agent_teams::teams();
        seats
            .iter()
            .filter_map(|seat| {
                let (team, pane) = seat.split_once('/')?;
                teams.get(team)?.term_of(pane)
            })
            .collect()
    };
    terms
        .into_iter()
        .filter_map(|term| host.worktree_of(term))
        .map(|tree| tree.to_string_lossy().into_owned())
        .collect()
}

/// The protection judgment this window stands behind the verb — the ONE place
/// it is chosen. A window that cannot say whose processes are whose ends
/// nothing.
///
/// The roots are the window's own reading of its panes ([`crate::kill_witness`]);
/// the processes the window (`window`, its pid) started that no pane holds —
/// its helpers — are added by the guard's own naming of them
/// ([`kill::ProtectedRoot::from_window_env`]), so a refusal says which was hit.
fn judge_with(
    roots: Option<KillRoots>,
    table: ProcessTable,
    checkouts: Vec<Checkout>,
    window: u32,
    asking: Vec<u32>,
    strays: Box<dyn Fn() -> Vec<sweep::Target>>,
) -> Box<dyn Judge> {
    match roots {
        Some(roots) => {
            let mut protected = roots.protected;
            let told = window.to_string();
            let named = kill::ProtectedRoot::from_window_env(
                |name| (name == kill::WINDOW_PID_ENV).then(|| told.clone()),
                &table,
                roots.caller.unwrap_or_default(),
            );
            for root in named {
                if !protected.iter().any(|held| held.pid == root.pid) {
                    protected.push(root);
                }
            }
            Box::new(KillRule {
                table,
                caller: roots.caller,
                protected,
                checkouts,
                asking,
                strays: std::cell::OnceCell::new(),
                read_strays: strays,
            })
        }
        None => Box::new(NoRoots { asking }),
    }
}

/// The kill guard's rule, asked of one pid at a time as a `kill <pid>` would
/// be: a process in the asking pane's own tree may end; one in the window's
/// or another pane's tree may not, whatever else is said; anything else is a
/// question for a person.
///
/// This verb asks nobody mid-call. The coordinator reading a list and naming
/// the pid@mark it showed is its yes to that question (t-78914, agreed with
/// t-80602's owner) — but only for an orphaned build: a process that stands
/// in a worker checkout the ledger knows, and whose every ancestor below pid
/// 1 stands in that same checkout. A runner's job (its parent the runner,
/// standing in the main checkout), a person's terminal (Terminal.app at `/`),
/// a chain the table cannot follow to pid 1, or anything else no pane holds
/// is refused as a question nobody can be asked. The guard's own rule is not
/// changed: the exception lives here.
///
/// A test's leftover is the second answer to that question (t-89544): after a
/// restart nothing above it stands in a worker checkout, so the first answer
/// refuses what the window itself would end. The boundary is not moved: this is
/// the window's own stray judgment, whole ([`crate::stray_reap`]), and the only
/// thing a named `pid@mark` stands in for is the ten minutes a freshly started
/// window has not yet watched.
struct KillRule {
    /// The same read the rows came from, in the guard's shape.
    table: ProcessTable,
    caller: Option<u32>,
    protected: Vec<kill::ProtectedRoot>,
    checkouts: Vec<Checkout>,
    /// The asking call and every process above it ([`asking_pids`]).
    asking: Vec<u32>,
    /// What the stray judgment names, read once and only if a row comes to it:
    /// it forks, and most calls never reach it.
    strays: std::cell::OnceCell<Vec<sweep::Target>>,
    read_strays: Box<dyn Fn() -> Vec<sweep::Target>>,
}

impl KillRule {
    /// Whether the window's stray judgment names this very process: same pid,
    /// same start, same command line.
    fn named_stray(&self, row: &ProcessRow) -> bool {
        let mark = sweep::mark(row.pid, &row.started, &row.args);
        self.strays
            .get_or_init(|| (self.read_strays)())
            .iter()
            .any(|stray| stray.pid == row.pid && stray.mark == mark)
    }

    /// The checkout an orphaned build stands in: `row` and every pid above
    /// it up to pid 1 all stand in this one. `None` when any of them stands
    /// elsewhere or cannot be placed, and when the guard's table does not
    /// follow the chain to pid 1 — an unread table, or a parent that left
    /// the table, proves nothing.
    fn orphaned_in(&self, row: &ProcessRow, rows: &[ProcessRow]) -> Option<&Checkout> {
        let owner = |cwd: Option<&str>| cwd.and_then(|cwd| sweep::owner(cwd, &self.checkouts));
        let home = owner(row.cwd.as_deref())?;
        let above = self.table.ancestors(row.pid);
        let (&top, below) = above.split_last()?;
        if top != 1 {
            return None;
        }
        for pid in below {
            let cwd = rows
                .iter()
                .find(|held| held.pid == *pid)
                .and_then(|held| held.cwd.as_deref());
            if owner(cwd)?.path != home.path {
                return None;
            }
        }
        Some(home)
    }
}

impl Judge for KillRule {
    /// `rows` is the same read as `self.table`, with the places the window
    /// read folded in; the guard's rule reads its own shape of it.
    fn judge(&self, row: &ProcessRow, rows: &[ProcessRow]) -> Judgment {
        if Some(row.pid) == self.caller {
            return Judgment::Deny {
                why: "the asking pane's own shell — ending it ends the call and its answer"
                    .to_string(),
            };
        }
        // Written as this verb: the guard quotes the call in its refusal, and a `kill` there names
        // a command nobody ran (r1-13).
        let call = KillCall {
            written: format!("{} --kill {}", sweep::VERB, row.pid),
            targets: vec![KillTarget::Pid(row.pid)],
        };
        // The guard's own refusal names what protects a process (a pane, the window), so it is
        // said first; what it would allow is refused when the process is above the asking call —
        // the coordinator's own tree is a pane's own tree, which the guard lets a pane signal
        // (r1-2).
        let above_the_call = || Judgment::Deny {
            why: "it is the asking call or a process above it — the shell and the agent this call \
                  runs in, up to the window; ending one ends the call and the answer it is \
                  waiting for"
                .to_string(),
        };
        let is_above = self.asking.contains(&row.pid);
        match kill::judge_kill(&[call], &self.table, self.caller, &self.protected) {
            KillVerdict::Allow if is_above => above_the_call(),
            KillVerdict::Allow => Judgment::Allow,
            KillVerdict::Deny { reason } => Judgment::Deny { why: reason },
            KillVerdict::Ask { .. } if is_above => above_the_call(),
            // For one pid the guard asks for two things: a process no pane
            // and not the window holds, or a table that did not read. Only
            // the first is the coordinator's to answer, and only for an
            // orphaned build; an unread table has no ancestry to follow, so
            // it stays a refusal.
            KillVerdict::Ask { reason } => match self.orphaned_in(row, rows) {
                Some(_) => Judgment::Allow,
                None if self.named_stray(row) => Judgment::Allow,
                None => Judgment::Deny {
                    why: format!(
                        "물을 수 없음 — {} asks nobody mid-call, and a process no pane holds is \
                         ended by its pid@mark only while it and every process above it up to \
                         pid 1 stand in one worker checkout the ledger knows; this one does not: \
                         {reason}",
                        sweep::VERB
                    ),
                },
            },
        }
    }

    fn asking(&self) -> &[u32] {
        &self.asking
    }

    /// The asked pids and every pid above them below pid 1, so the chain's
    /// places come from the same one read.
    fn places_for(&self, pids: &[u32]) -> Vec<u32> {
        let mut places = pids.to_vec();
        for pid in pids {
            for above in self.table.ancestors(*pid) {
                if above > 1 && !places.contains(&above) {
                    places.push(above);
                }
            }
        }
        places
    }
}

struct NoRoots {
    asking: Vec<u32>,
}

impl Judge for NoRoots {
    fn asking(&self) -> &[u32] {
        &self.asking
    }

    fn judge(&self, _row: &ProcessRow, _table: &[ProcessRow]) -> Judgment {
        Judgment::Deny {
            why: "this window could not say which processes are its panes', so nothing is ended"
                .to_string(),
        }
    }
}

/// `checkouts` are the ask's, less every coordinator's tree — the same list
/// the judge holds, so a row's owner and its verdict read one list.
fn carry_with(
    ask: &ProcessSweepAsk,
    checkouts: &[Checkout],
    mut table: Vec<ProcessRow>,
    read_cwds: &dyn Fn(&[u32]) -> HashMap<u32, String>,
    judge: &dyn Judge,
    signals: &dyn Signals,
) -> Result<Carried, String> {
    match ask {
        ProcessSweepAsk::Find { needle, .. } => {
            let cwds =
                read_cwds(&judge.places_for(&sweep::matching_pids(&table, needle, judge.asking())));
            sweep::with_cwds(&mut table, &cwds);
            Ok(Carried {
                said: sweep::find(&table, needle, checkouts, judge).to_string(),
                filed: None,
            })
        }
        ProcessSweepAsk::End {
            run,
            generation,
            request,
            targets,
            ..
        } => {
            let cwds = read_cwds(&judge.places_for(&sweep::target_pids(targets)));
            sweep::with_cwds(&mut table, &cwds);
            let ended = sweep::end(&table, targets, judge, signals);
            Ok(Carried {
                said: sweep::end_answer(run, &ended).to_string(),
                filed: Some(Filed {
                    run: run.clone(),
                    line: sweep::ledger_line(*generation, &ended),
                    receipt: sweep::receipt_of(run, request, targets),
                }),
            })
        }
    }
}

/// The table, every row that carries both halves of its identity, and the
/// same rows in the guard's shape. An empty read is a failed read — this
/// process itself is always in the table — and nothing is listed or ended on
/// it.
fn table() -> Result<(Vec<ProcessRow>, ProcessTable), String> {
    let sample = crate::resource_usage::enumerate_processes()?;
    let mut rows = Vec::new();
    let mut guarded = Vec::new();
    for process in sample.identified() {
        rows.push(ProcessRow {
            pid: process.pid,
            ppid: process.ppid,
            started: process.started.to_string(),
            args: process.command.to_string(),
            cwd: None,
        });
        guarded.push(kill::ProcessRow {
            pid: process.pid,
            ppid: process.ppid,
            pgid: process.pgid,
            command: process.command.to_string(),
        });
    }
    if rows.is_empty() {
        return Err("the process table read empty, so nothing is listed or ended".to_string());
    }
    Ok((rows, ProcessTable::new(guarded)))
}

/// The real system, for one call: its clock starts when the call does.
pub(crate) struct System {
    began: std::time::Instant,
}

impl System {
    pub(crate) fn new() -> Self {
        Self {
            began: std::time::Instant::now(),
        }
    }
}

impl Signals for System {
    /// Every asked pid in one `ps` (r2 P2-1); Windows reads one at a time.
    #[cfg(not(windows))]
    fn reread_many(&self, pids: &[u32]) -> Option<HashMap<u32, (String, String)>> {
        crate::resource_usage::process_identities(pids).ok()
    }

    fn reread(&self, pid: u32) -> Option<(String, String)> {
        let started = crate::resource_usage::process_start_identity(pid).ok()?;
        let args = crate::resource_usage::process_command(pid).ok()?;
        // The start is read on both sides of the command line, so a pid handed
        // to another process between the two reads cannot pair one process's
        // start with the other's arguments.
        let again = crate::resource_usage::process_start_identity(pid).ok()?;
        (again == started && !args.is_empty()).then_some((started, args))
    }

    fn terminate(&self, pid: u32) -> Result<(), String> {
        crate::resource_usage::end_one_process(pid)
    }

    fn pause(&self, ms: u64) {
        std::thread::sleep(std::time::Duration::from_millis(ms));
    }

    fn now_ms(&self) -> u64 {
        u64::try_from(self.began.elapsed().as_millis()).unwrap_or(u64::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_host::absolute_text;
    use std::cell::RefCell;
    use zerocode_core::orchestration::process_sweep::{Target, mark};

    /// The judge over a window whose stray judgment names nothing.
    fn judge(
        roots: Option<KillRoots>,
        table: ProcessTable,
        checkouts: Vec<Checkout>,
        window: u32,
        asking: Vec<u32>,
    ) -> Box<dyn Judge> {
        judge_with(roots, table, checkouts, window, asking, Box::new(Vec::new))
    }

    const STARTED: &str = "Wed Oct 28 21:00:00 2026";

    fn row(pid: u32, args: &str) -> ProcessRow {
        ProcessRow {
            pid,
            ppid: 1,
            started: STARTED.to_string(),
            args: args.to_string(),
            cwd: None,
        }
    }

    fn target(pid: u32, args: &str) -> Target {
        Target {
            pid,
            mark: mark(pid, STARTED, args),
        }
    }

    fn no_cwds(_: &[u32]) -> HashMap<u32, String> {
        HashMap::new()
    }

    struct Allow;

    impl Judge for Allow {
        fn judge(&self, _row: &ProcessRow, _table: &[ProcessRow]) -> Judgment {
            Judgment::Allow
        }
    }

    /// Answers every reread with the table's own row, and writes down each
    /// pid it was asked to end instead of signalling anything; a written-down
    /// pid leaves the table, as a process that ends on TERM does.
    struct Recorded {
        table: RefCell<Vec<ProcessRow>>,
        ended: RefCell<Vec<u32>>,
    }

    impl Recorded {
        fn over(table: &[ProcessRow]) -> Self {
            Self {
                table: RefCell::new(table.to_vec()),
                ended: RefCell::new(Vec::new()),
            }
        }
    }

    impl Signals for Recorded {
        fn reread(&self, pid: u32) -> Option<(String, String)> {
            self.table
                .borrow()
                .iter()
                .find(|row| row.pid == pid)
                .map(|row| (row.started.clone(), row.args.clone()))
        }

        fn terminate(&self, pid: u32) -> Result<(), String> {
            self.ended.borrow_mut().push(pid);
            self.table.borrow_mut().retain(|row| row.pid != pid);
            Ok(())
        }

        fn pause(&self, _ms: u64) {}

        fn now_ms(&self) -> u64 {
            0
        }
    }

    fn end_call(targets: Vec<Target>) -> ProcessSweepAsk {
        end_call_over(targets, Vec::new())
    }

    fn end_call_over(targets: Vec<Target>, checkouts: Vec<Checkout>) -> ProcessSweepAsk {
        ProcessSweepAsk::End {
            run: "run-1".to_string(),
            generation: 3,
            request: "sweep-1".to_string(),
            targets,
            checkouts,
            project_roots: Vec::new(),
            coordinator_seats: Vec::new(),
        }
    }

    /// A list asks where only its own matches stand, and says the place.
    #[test]
    fn a_list_reads_the_places_of_its_matches_only() {
        let fixture = vec![
            row(4100, "cargo build -p zerocode-shell"),
            row(4200, "node server.js"),
        ];
        let asked = RefCell::new(Vec::new());
        let read_cwds = |pids: &[u32]| {
            asked.borrow_mut().extend_from_slice(pids);
            HashMap::from([(4100, "/w/t-1".to_string())])
        };
        let signals = Recorded::over(&fixture);
        let carried = carry_with(
            &ProcessSweepAsk::Find {
                needle: "cargo".to_string(),
                checkouts: Vec::new(),
                project_roots: Vec::new(),
                coordinator_seats: Vec::new(),
            },
            &[],
            fixture,
            &read_cwds,
            &Allow,
            &signals,
        )
        .expect("a list");
        assert_eq!(*asked.borrow(), vec![4100]);
        let said: serde_json::Value = serde_json::from_str(&carried.said).expect("JSON");
        assert_eq!(said["rows"][0]["cwd"], "/w/t-1");
        assert!(carried.filed.is_none(), "a list files nothing");
        assert!(signals.ended.borrow().is_empty(), "a list signals nothing");
    }

    /// An end call answers and files one line for its run, under a receipt.
    #[test]
    fn an_end_call_files_one_line_for_its_run() {
        let args = "cargo build -p zerocode-shell";
        let fixture = vec![row(4100, args)];
        let signals = Recorded::over(&fixture);
        let carried = carry_with(
            &end_call(vec![target(4100, args)]),
            &[],
            fixture,
            &no_cwds,
            &Allow,
            &signals,
        )
        .expect("an end call");
        assert_eq!(*signals.ended.borrow(), vec![4100]);
        let filed = carried.filed.expect("a line for the ledger");
        assert_eq!(filed.run, "run-1");
        assert!(
            filed.line.starts_with(
                "process-sweep: TERM sent to 1 of 1, 1 gone, asked by coordinator generation 3"
            ),
            "{}",
            filed.line
        );
        // Filed under the call's own retry name, never a clock: carried
        // twice, one call files one line (r2 P3).
        assert_eq!(
            filed.receipt,
            sweep::receipt_of("run-1", "sweep-1", &[target(4100, args)])
        );
        assert!(filed.receipt.contains("sweep-1"), "{}", filed.receipt);
    }

    /// A window that cannot say whose processes are whose ends nothing.
    #[test]
    fn without_the_panes_roots_nothing_is_ended() {
        let fixture = vec![row(4100, "cargo build")];
        let signals = Recorded::over(&fixture);
        let judge = judge(
            None,
            ProcessTable::new(Vec::new()),
            Vec::new(),
            500,
            Vec::new(),
        );
        let carried = carry_with(
            &end_call(vec![target(4100, "cargo build")]),
            &[],
            fixture,
            &no_cwds,
            judge.as_ref(),
            &signals,
        )
        .expect("an end call");
        assert!(signals.ended.borrow().is_empty());
        let said: serde_json::Value = serde_json::from_str(&carried.said).expect("JSON");
        assert_eq!(said["rows"][0]["outcome"], "refused");
    }

    /// One machine as `ps -o pid=,ppid=,pgid=,command=` prints it: the window
    /// (500), the asking pane (600) with its agent and build, another pane
    /// (610) with a build in a worker checkout, an orphaned build in that
    /// checkout, an orphaned build-line runner outside every checkout and the
    /// job it started (4410, its cargo 4420) standing in the checkout; a
    /// person's Terminal (800) whose shell (820) and build (830) stand in the
    /// checkout; an orphaned cargo (5500) with its rustc (5510), both in the
    /// checkout; a rustc whose parent left the table (5600); and a build
    /// (5710) under an orphan (5700) standing in another worker's checkout.
    const LISTING: &str = "\
        500 1 500 /Applications/ZeroCode.app/Contents/MacOS/zerocode-shell\n\
        600 500 600 -zsh\n\
        700 600 600 claude --model opus\n\
        4100 700 4100 cargo build -p zerocode-shell\n\
        610 500 610 -zsh\n\
        4200 610 4200 cargo test -p zerocode-core\n\
        4300 1 4300 rustc --crate-name zerocode_shell\n\
        4400 1 4400 zsh build-queue/runner10.sh 1\n\
        4410 4400 4410 zsh /q/run/gate.job\n\
        4420 4410 4410 cargo test -p zerocode-shell\n\
        800 1 800 /System/Applications/Utilities/Terminal.app/Contents/MacOS/Terminal\n\
        810 800 810 login -pf person\n\
        820 810 820 -zsh\n\
        830 820 830 cargo build\n\
        5500 1 5500 cargo build -p zerocode-core\n\
        5510 5500 5500 rustc --crate-name zerocode_core\n\
        5600 9999 5600 rustc --crate-name zerocode_pty\n\
        5700 1 5700 cargo build -p zerocode-pty\n\
        5710 5700 5700 rustc --crate-name zerocode_pty\n";

    /// Where each stands: the other pane's build, the orphaned builds, the
    /// runner's job, the person's shell and build all in worker w-9's
    /// checkout; the runner outside every checkout, Terminal at the root;
    /// 5700 in worker w-8's checkout, its rustc in w-9's.
    fn places(pids: &[u32]) -> HashMap<u32, String> {
        let all = [
            (4100, "/w/coordinator"),
            (4200, "/w/t-9"),
            (4300, "/w/t-9/crates/zerocode-shell"),
            (4400, "/build-line"),
            (4410, "/w/t-9"),
            (4420, "/w/t-9"),
            (800, "/"),
            (820, "/w/t-9"),
            (830, "/w/t-9"),
            (5500, "/w/t-9"),
            (5510, "/w/t-9/crates/zerocode-core"),
            (5600, "/w/t-9"),
            (5700, "/w/t-8"),
            (5710, "/w/t-9"),
        ];
        all.into_iter()
            .filter(|(pid, _)| pids.contains(pid))
            .map(|(pid, cwd)| (pid, absolute_text(cwd)))
            .collect()
    }

    /// Ends `pids` on [`LISTING`] through the kill guard's real rule, with
    /// the window's roots as the guard reads them; nothing real is signalled.
    /// Answers the pids signalled and the answer's rows.
    fn guarded_end(pids: &[u32]) -> (Vec<u32>, Vec<serde_json::Value>) {
        guarded_end_over(LISTING, ProcessTable::from_listing(LISTING, None), pids)
    }

    /// The same over `listing`, with the guard's table given — read, or not.
    fn guarded_end_over(
        listing: &str,
        guard_table: ProcessTable,
        pids: &[u32],
    ) -> (Vec<u32>, Vec<serde_json::Value>) {
        guarded_end_told(listing, guard_table, guarded_checkouts(), &[], pids)
    }

    /// The same with the worker checkouts the ledger `told` and the trees
    /// the window read for the live coordinator seats, as [`carry`] holds
    /// them ([`held_checkouts`]).
    fn guarded_end_told(
        listing: &str,
        guard_table: ProcessTable,
        told: Vec<Checkout>,
        coordinator_trees: &[String],
        pids: &[u32],
    ) -> (Vec<u32>, Vec<serde_json::Value>) {
        guarded_end_named(
            listing,
            guard_table,
            told,
            coordinator_trees,
            pids,
            Vec::new(),
        )
    }

    /// The same with the window's stray judgment naming `named`.
    fn guarded_end_named(
        listing: &str,
        guard_table: ProcessTable,
        told: Vec<Checkout>,
        coordinator_trees: &[String],
        pids: &[u32],
        named: Vec<Target>,
    ) -> (Vec<u32>, Vec<serde_json::Value>) {
        let rows: Vec<ProcessRow> = listing
            .lines()
            .map(|line| {
                let mut fields = line.split_whitespace();
                let pid: u32 = fields.next().expect("pid").parse().expect("a pid");
                let ppid: u32 = fields.next().expect("ppid").parse().expect("a ppid");
                let _pgid = fields.next();
                let args = fields.collect::<Vec<_>>().join(" ");
                ProcessRow {
                    ppid,
                    ..row(pid, &args)
                }
            })
            .collect();
        let targets: Vec<Target> = pids
            .iter()
            .map(|pid| {
                let held = rows.iter().find(|row| row.pid == *pid).expect("a row");
                target(*pid, &held.args)
            })
            .collect();
        let ask = end_call_over(targets, told);
        let checkouts = held_checkouts(&ask, coordinator_trees);
        let judge = judge_with(
            Some(guarded_roots()),
            guard_table,
            checkouts.clone(),
            500,
            Vec::new(),
            Box::new(move || named.clone()),
        );
        let signals = Recorded::over(&rows);
        let carried = carry_with(&ask, &checkouts, rows, &places, judge.as_ref(), &signals)
            .expect("an end call");
        let said: serde_json::Value = serde_json::from_str(&carried.said).expect("JSON");
        let rows = said["rows"].as_array().expect("rows").clone();
        let signalled = signals.ended.borrow().clone();
        (signalled, rows)
    }

    /// The window's roots as the guard reads them: the asking pane 600, the
    /// window 500 and another pane 610.
    fn guarded_roots() -> KillRoots {
        KillRoots {
            caller: Some(600),
            caller_mark: None,
            protected: vec![
                kill::ProtectedRoot {
                    pid: 500,
                    label: "the ZeroCode window".to_string(),
                },
                kill::ProtectedRoot {
                    pid: 610,
                    label: "pane term-3".to_string(),
                },
            ],
        }
    }

    fn guarded_checkouts() -> Vec<Checkout> {
        ["/w/t-9", "/w/t-8"]
            .into_iter()
            .map(|path| told(&absolute_text(path)))
            .collect()
    }

    /// A worker checkout at `path`, as the ledger tells it.
    fn told(path: &str) -> Checkout {
        let at = path.rsplit('-').next().unwrap_or_default();
        Checkout {
            path: path.to_string(),
            run: "run-1".to_string(),
            worker: format!("w-{at}"),
            task: Some(format!("t-{at}")),
            started_ms: 0,
        }
    }

    fn why(row: &serde_json::Value) -> &str {
        row["why"].as_str().unwrap_or_default()
    }

    /// The orphaned rustc of 03:23, named by the pid@mark a list showed and
    /// standing in a worker checkout, is ended — the coordinator's pick is
    /// the yes the guard would ask for. The asking pane's own build ends too.
    #[test]
    fn an_orphaned_build_named_by_its_mark_is_ended() {
        let (signalled, rows) = guarded_end(&[4100, 4300]);
        assert_eq!(signalled, vec![4100, 4300]);
        assert_eq!(rows[0]["outcome"], "signaled");
        assert_eq!(rows[1]["outcome"], "signaled");
        assert_eq!(rows[1]["after"], "gone");
    }

    /// An orphaned cargo and the rustc under it, every process up to pid 1
    /// standing in the one checkout, are ended — the tree 03:23 left (r1
    /// P1-2). The places read for them are theirs and their ancestors'.
    #[test]
    fn an_orphaned_tree_wholly_in_one_checkout_is_ended() {
        let (signalled, rows) = guarded_end(&[5510, 5500]);
        assert_eq!(signalled, vec![5510, 5500], "{rows:?}");
        let table = ProcessTable::from_listing(LISTING, None);
        let judge = judge(
            Some(guarded_roots()),
            table,
            guarded_checkouts(),
            500,
            Vec::new(),
        );
        assert_eq!(judge.places_for(&[5510]), vec![5510, 5500]);
        assert_eq!(judge.places_for(&[4420]), vec![4420, 4410, 4400]);
    }

    /// A process standing in a worker checkout whose chain leaves it is no
    /// orphaned build (r1 P1-2): a build-line runner's job (its runner stands
    /// in the main checkout), a person's Terminal shell and its build
    /// (Terminal stands at `/`), a build under an orphan in another worker's
    /// checkout and a rustc whose parent left the table are all refused as
    /// questions nobody can be asked.
    #[test]
    fn a_checkout_process_whose_chain_leaves_the_checkout_is_refused() {
        let (signalled, rows) = guarded_end(&[4410, 4420, 820, 830, 5710, 5600]);
        assert!(signalled.is_empty(), "{signalled:?}");
        for row in &rows {
            assert_eq!(row["outcome"], "refused", "{row}");
            assert!(why(row).starts_with("물을 수 없음"), "{row}");
        }
    }

    /// The asking pane's own shell is refused: ending it ends the call
    /// before its answer and its ledger line.
    #[test]
    fn the_asking_panes_own_shell_is_refused() {
        let (signalled, rows) = guarded_end(&[600]);
        assert!(signalled.is_empty(), "{signalled:?}");
        assert!(why(&rows[0]).contains("own shell"), "{rows:?}");
    }

    /// Another pane's build is refused by name though its pid@mark is right
    /// and it stands in a worker checkout; so is the window. No exception
    /// reaches inside a pane or the window.
    #[test]
    fn a_protected_panes_build_is_refused_even_with_its_mark() {
        let (signalled, rows) = guarded_end(&[4200, 500]);
        assert!(signalled.is_empty(), "{signalled:?}");
        assert_eq!(rows[0]["outcome"], "refused");
        assert!(why(&rows[0]).contains("pane term-3"), "{rows:?}");
        assert_eq!(rows[1]["outcome"], "refused");
        assert!(why(&rows[1]).contains("the ZeroCode window"), "{rows:?}");
    }

    /// A refusal does not say `kill` (r1-13): this verb signals with TERM through its own road, and
    /// a coordinator reading "`kill 500` would signal …" looks for a `kill` it never ran. The
    /// guard's reason quotes the call as it was written, so the call is written as this verb.
    #[test]
    fn a_refusal_names_this_verb_and_never_a_kill_command() {
        let (_, rows) = guarded_end(&[4200, 500, 4400]);
        assert_eq!(rows.len(), 3, "{rows:?}");
        for row in &rows {
            assert!(!why(row).contains("`kill"), "{row}");
        }
        assert!(why(&rows[0]).contains("process-sweep"), "{rows:?}");
        assert!(why(&rows[1]).contains("process-sweep"), "{rows:?}");
    }

    /// The listing of a coordinator that runs `eval 'zerocode-orc process-sweep …'` from a tool
    /// shell: the window (500), the pane (600), the agent (700), the tool's shell (710), the
    /// shim (720) and its subshell (721) with the `curl` (722) that carries the call, a second
    /// pane's call (820, under 810) and a build of the agent's own (4100).
    const CALLING: &str = "\
        500 1 500 /Applications/ZeroCode.app/Contents/MacOS/zerocode-shell\n\
        600 500 600 -zsh\n\
        700 600 600 claude --model opus\n\
        710 700 710 /bin/zsh -c source ~/.zshrc && eval 'zerocode-orc process-sweep --find cargo'\n\
        720 710 710 sh /w/orchestration-bin/zerocode-orc process-sweep --find cargo\n\
        721 720 710 sh /w/orchestration-bin/zerocode-orc process-sweep --find cargo\n\
        722 721 710 curl -q -sS --noproxy * --proto =http --max-time 12 -w \\n%{http_code} -K /dev/fd/3 -H x-zerocode-team-id: team-a -H x-zerocode-team-pane: %1 -H content-type: application/octet-stream --data-binary @- http://127.0.0.1:52506/agent-teams\n\
        810 500 810 -zsh\n\
        820 810 810 curl -q -sS --noproxy * --proto =http --max-time 12 -w \\n%{http_code} -K /dev/fd/3 -H x-zerocode-team-id: team-a -H x-zerocode-team-pane: %10 -H content-type: application/octet-stream --data-binary @- http://127.0.0.1:52506/agent-teams\n\
        4100 700 600 cargo build -p zerocode-shell\n";

    fn calling_rows() -> Vec<ProcessRow> {
        CALLING
            .lines()
            .map(|line| {
                let mut fields = line.split_whitespace();
                let pid: u32 = fields.next().expect("pid").parse().expect("a pid");
                let ppid: u32 = fields.next().expect("ppid").parse().expect("a ppid");
                let _pgid = fields.next();
                let args = fields.collect::<Vec<_>>().join(" ");
                ProcessRow {
                    ppid,
                    ..row(pid, &args)
                }
            })
            .collect()
    }

    /// The asking call is the `curl` that names this team and pane, and everything above it is
    /// found by the parent each row names (t-84800 r2 P2-2): the tool's shell is above it
    /// whatever its command line spells, and another pane's call is not this one's, though its pane
    /// begins with the same characters.
    #[test]
    fn the_asking_call_and_everything_above_it_are_found_by_where_they_stand() {
        let rows = calling_rows();
        assert_eq!(
            asking_pids(&rows, "team-a", "%1"),
            vec![500, 600, 700, 710, 720, 721, 722]
        );
        assert_eq!(asking_pids(&rows, "team-a", "%10"), vec![500, 810, 820]);
        assert!(asking_pids(&rows, "team-b", "%1").is_empty());
        assert!(asking_pids(&rows, "", "%1").is_empty());
        assert!(asking_pids(&rows, "team-a", "").is_empty());
    }

    /// A list hides the processes above the asking call whatever their command lines spell, and
    /// shows the agent's own build (r1-2, P2-2).
    #[test]
    fn a_list_hides_the_processes_above_its_own_call() {
        let rows = calling_rows();
        let asking = asking_pids(&rows, "team-a", "%1");
        let judge = judge(
            Some(guarded_roots()),
            ProcessTable::from_listing(CALLING, None),
            Vec::new(),
            500,
            asking,
        );
        let ask = ProcessSweepAsk::Find {
            needle: "c".to_string(),
            checkouts: Vec::new(),
            project_roots: Vec::new(),
            coordinator_seats: Vec::new(),
        };
        let signals = Recorded::over(&rows);
        let carried =
            carry_with(&ask, &[], rows, &no_cwds, judge.as_ref(), &signals).expect("a list");
        let said: serde_json::Value = serde_json::from_str(&carried.said).expect("JSON");
        let shown: Vec<u64> = said["rows"]
            .as_array()
            .expect("rows")
            .iter()
            .filter_map(|row| row["pid"].as_u64())
            .collect();
        // 820 (another pane's call) and 4100 (the agent's build) stand; 500-722 do not.
        assert_eq!(shown, vec![820, 4100], "{said}");
    }

    /// The agent a coordinator runs in is a pane's own process, which the guard lets a pane signal;
    /// it and the shells between it and the call are refused as the call's own tree (r1-2), and the
    /// agent's build beside them is ended as before.
    #[test]
    fn the_coordinators_own_tree_above_the_call_is_refused() {
        let rows = calling_rows();
        let asking = asking_pids(&rows, "team-a", "%1");
        let judge = judge(
            Some(guarded_roots()),
            ProcessTable::from_listing(CALLING, None),
            Vec::new(),
            500,
            asking,
        );
        for pid in [700, 710, 720, 722] {
            let row = rows.iter().find(|row| row.pid == pid).expect("a row");
            match judge.judge(row, &rows) {
                Judgment::Deny { why } => assert!(why.contains("above"), "{pid}: {why}"),
                Judgment::Allow => panic!("{pid} may be ended: the coordinator's own tree"),
            }
        }
        let build = rows.iter().find(|row| row.pid == 4100).expect("a row");
        assert!(matches!(judge.judge(build, &rows), Judgment::Allow));
    }

    /// The main checkout is taken out again after every path is read as the system spells it
    /// (t-84800 r2 P2-3): a worker's path that reaches the main checkout through a link above it
    /// passes the ledger's comparison of told paths.
    #[cfg(unix)]
    #[test]
    fn the_main_checkout_reached_through_a_link_above_is_nobodys() {
        let dir = tempfile::tempdir().expect("scratch");
        let root = dir.path().canonicalize().expect("real");
        let main = root.join("area/main");
        std::fs::create_dir_all(main.join(".git")).expect("main checkout");
        let worker = root.join("area/t-9");
        std::fs::create_dir_all(&worker).expect("worker checkout");
        let through = root.join("through");
        std::os::unix::fs::symlink(root.join("area"), &through).expect("link");
        let mut main_told = told(&through.join("main").display().to_string());
        main_told.started_ms = i64::MAX / 4;
        let mut worker_told = told(&through.join("t-9").display().to_string());
        worker_told.started_ms = i64::MAX / 4;
        let mut ask = end_call_over(Vec::new(), vec![main_told, worker_told]);
        if let ProcessSweepAsk::End { project_roots, .. } = &mut ask {
            project_roots.push(root.join("area/main/.git").display().to_string());
        }
        let held = held_checkouts(&ask, &[]);
        let paths: Vec<&str> = held.iter().map(|one| one.path.as_str()).collect();
        assert_eq!(paths, vec![worker.display().to_string()], "{held:?}");
    }

    /// A process no pane holds that stands in no worker checkout — a
    /// build-line runner — is a question nobody can be asked: refused.
    #[test]
    fn an_orphan_outside_every_checkout_is_refused_as_unaskable() {
        let (signalled, rows) = guarded_end(&[4400]);
        assert!(signalled.is_empty(), "{signalled:?}");
        assert_eq!(rows[0]["outcome"], "refused");
        assert!(why(&rows[0]).starts_with("물을 수 없음"), "{rows:?}");
    }

    /// After a restart a test's leftover stands in no worker checkout the ledger
    /// knows, so the orphaned-build answer refuses it (11-02 10:32, t-89544). The
    /// window's own stray judgment is the second answer: what it names, by the
    /// same pid@mark, is ended; what it does not name is refused as before, and a
    /// pane's or the window's process is refused whatever it names.
    #[test]
    fn a_test_leftover_the_stray_judgment_names_is_ended_and_nothing_else() {
        let guard = || ProcessTable::from_listing(LISTING, None);
        let (signalled, rows) = guarded_end(&[4400]);
        assert!(
            signalled.is_empty(),
            "the red: refused without the judgment"
        );
        assert_eq!(rows[0]["outcome"], "refused");

        let named = vec![target(4400, "zsh build-queue/runner10.sh 1")];
        let (signalled, rows) =
            guarded_end_named(LISTING, guard(), guarded_checkouts(), &[], &[4400], named);
        assert_eq!(signalled, vec![4400], "{rows:?}");

        let stale = vec![Target {
            pid: 4400,
            mark: mark(4400, "another start", "zsh build-queue/runner10.sh 1"),
        }];
        let (signalled, _) =
            guarded_end_named(LISTING, guard(), guarded_checkouts(), &[], &[4400], stale);
        assert!(signalled.is_empty(), "another process under the same pid");

        let in_a_pane = vec![target(4200, "cargo test -p zerocode-core")];
        let (signalled, rows) = guarded_end_named(
            LISTING,
            guard(),
            guarded_checkouts(),
            &[],
            &[4200],
            in_a_pane,
        );
        assert!(signalled.is_empty(), "a pane's own: {rows:?}");
        let window = vec![target(
            500,
            "/Applications/ZeroCode.app/Contents/MacOS/zerocode-shell",
        )];
        let (signalled, _) =
            guarded_end_named(LISTING, guard(), guarded_checkouts(), &[], &[500], window);
        assert!(signalled.is_empty(), "the window");
    }

    /// When the guard's table did not read, the window alone is kept and the
    /// guard asks about everything; that question is not the coordinator's to
    /// answer, so even an orphaned build in a worker checkout is refused.
    #[test]
    fn an_unread_table_ends_nothing_even_in_a_checkout() {
        let (signalled, rows) =
            guarded_end_over(LISTING, ProcessTable::unreadable("ps timed out"), &[4300]);
        assert!(signalled.is_empty(), "{signalled:?}");
        assert_eq!(rows[0]["outcome"], "refused");
        assert!(why(&rows[0]).starts_with("물을 수 없음"), "{rows:?}");
    }

    /// The window's helpers — processes it started that no pane holds — are
    /// refused by the guard's own naming of them.
    #[test]
    fn a_window_helper_is_refused_by_name() {
        let listing = format!("{LISTING}4500 500 4500 zerocode-cef-helper --type=gpu\n");
        let (signalled, rows) = guarded_end_over(
            &listing,
            ProcessTable::from_listing(&listing, None),
            &[4500],
        );
        assert!(signalled.is_empty(), "{signalled:?}");
        assert_eq!(rows[0]["outcome"], "refused");
        assert!(why(&rows[0]).contains("zerocode-cef-helper"), "{rows:?}");
    }
    /// r2 P1-1: a worker opened without a tree of its own reports its
    /// coordinator's tree as its checkout — on this machine 191 of 1,499
    /// worker rows named the main checkout, where the build line's runner
    /// (ppid 1) stands. Whatever the ledger told, a checkout that is or
    /// holds a live coordinator seat's tree is nobody's, so the runner there
    /// is refused as a question nobody can be asked. An orphaned build in a
    /// worker's own tree is still ended.
    #[test]
    fn a_runner_in_a_coordinators_tree_is_refused_whatever_a_row_says() {
        let mut told_rows = guarded_checkouts();
        told_rows.push(told(&absolute_text("/build-line")));
        for tree in ["/build-line", "/build-line/crates/zerocode-shell"] {
            let (signalled, rows) = guarded_end_told(
                LISTING,
                ProcessTable::from_listing(LISTING, None),
                told_rows.clone(),
                &[absolute_text(tree)],
                &[4400, 5510, 5500],
            );
            assert_eq!(signalled, vec![5510, 5500], "{tree}: {rows:?}");
            assert_eq!(rows[0]["outcome"], "refused", "{tree}: {rows:?}");
            assert!(why(&rows[0]).starts_with("물을 수 없음"), "{rows:?}");
        }
    }

    /// The control for the case above: the same row told as a worker's own
    /// tree, with no coordinator standing in it, is an orphan's checkout —
    /// so it is the coordinator's tree, and only that, that refuses.
    #[test]
    fn a_checkout_no_coordinator_stands_in_still_holds_its_orphan() {
        let mut told_rows = guarded_checkouts();
        let runner_tree = absolute_text("/build-line");
        let coordinator = absolute_text("/w/coordinator");
        told_rows.push(told(&runner_tree));
        let ask = end_call_over(Vec::new(), told_rows.clone());
        let held = held_checkouts(&ask, std::slice::from_ref(&coordinator));
        assert!(held.iter().any(|one| one.path == runner_tree), "{held:?}");
        // And through the whole call (r3-P3-6): with no coordinator in that tree, the orphan
        // there is ended, the one pid asked and no other.
        let (signalled, rows) = guarded_end_told(
            LISTING,
            ProcessTable::from_listing(LISTING, None),
            told_rows,
            std::slice::from_ref(&coordinator),
            &[4400],
        );
        assert_eq!(signalled, vec![4400], "{rows:?}");
        assert_eq!(rows[0]["outcome"], "signaled", "{rows:?}");
        let held = held_checkouts(&ask, std::slice::from_ref(&runner_tree));
        let paths: Vec<String> = held.iter().map(|one| one.path.clone()).collect();
        assert_eq!(
            paths,
            vec![absolute_text("/w/t-9"), absolute_text("/w/t-8")]
        );
    }

    /// 끝나지 않는 자식은 시간 안에 죽고 `None` 이다 — 창의 답이 그 자식을 기다리지 않는다.
    /// 시간 안에 끝난 자식은 종료 코드가 0이 아니어도 출력을 돌려준다(lsof 는 보고할 것이
    /// 없을 때 0이 아닌 코드로 끝난다).
    #[cfg(unix)]
    #[test]
    fn a_child_that_does_not_finish_is_ended_and_answers_none() {
        let began = std::time::Instant::now();
        let mut slow = crate::proc::quiet_command("sleep");
        slow.arg("3");
        assert!(
            crate::system_runtime::output_within(slow, std::time::Duration::from_millis(150))
                .is_none()
        );
        assert!(began.elapsed() < std::time::Duration::from_millis(2_500));
        let mut quick = crate::proc::quiet_command("sh");
        quick.args(["-c", "echo hello; exit 1"]);
        let output = crate::system_runtime::output_within(quick, std::time::Duration::from_secs(5))
            .expect("it finished");
        assert_eq!(String::from_utf8_lossy(&output.stdout), "hello\n");
        assert!(!output.status.success());
    }

    /// A worker's folder that is now a link is nobody's checkout (t-84800 r2 P2-3): the link may
    /// lead to an older folder — the main checkout — and every check that follows it would read the
    /// main checkout as the worker's, so a process standing there looks like a worker's orphan.
    /// `git worktree add` never makes a link; a link as the last part of the path is not a worktree.
    #[cfg(unix)]
    #[test]
    fn a_checkout_that_is_now_a_link_is_nobodys() {
        let dir = tempfile::tempdir().expect("scratch");
        let root = dir.path().canonicalize().expect("real");
        let main = root.join("main");
        std::fs::create_dir_all(main.join(".git")).expect("main checkout");
        let swapped = root.join("t-9");
        std::os::unix::fs::symlink(&main, &swapped).expect("link");
        let mut taken = told(&swapped.display().to_string());
        taken.started_ms = i64::MAX / 4;
        let ask = end_call_over(Vec::new(), vec![taken]);
        let held = held_checkouts(&ask, &[]);
        assert!(held.is_empty(), "{held:?}");
    }

    /// A child killed at its deadline is not waited for (t-84800 r2 P3-1): a helper it started holds
    /// the pipe open after the child is gone, and a read to the end would wait for the helper.
    #[cfg(unix)]
    #[test]
    fn a_killed_child_whose_helper_holds_the_pipe_is_not_waited_for() {
        let began = std::time::Instant::now();
        let mut command = crate::proc::quiet_command("sh");
        command.args(["-c", "sleep 3 & exec sleep 3"]);
        assert!(
            crate::system_runtime::output_within(command, std::time::Duration::from_millis(150))
                .is_none()
        );
        assert!(
            began.elapsed() < std::time::Duration::from_millis(1_500),
            "waited {:?}",
            began.elapsed()
        );
    }

    /// A scan that answered nothing is not always an unsupported platform (t-84800 r2 P3-3): on
    /// macOS the bounded `lsof` answers `None` at its deadline too, and the screen says both.
    #[test]
    fn an_unread_port_scan_does_not_blame_the_platform_alone() {
        let said = crate::system_runtime::listeners_unread("macos");
        assert!(said.contains("macos"), "{said}");
        assert!(said.contains("시간"), "{said}");
        assert!(said.contains("플랫폼"), "{said}");
    }

    /// A worker's checkout told through a link is the folder the system says its processes
    /// stand in (r1-10): `lsof` prints the real place, and a tree told the other way round
    /// still takes its checkout out.
    #[cfg(unix)]
    #[test]
    fn a_checkout_told_through_a_link_is_read_as_the_real_folder() {
        let dir = tempfile::tempdir().expect("scratch");
        let real = dir.path().canonicalize().expect("real").join("real");
        std::fs::create_dir_all(real.join("t-9/crates")).expect("folders");
        let link = dir.path().canonicalize().expect("real").join("link");
        std::os::unix::fs::symlink(&real, &link).expect("link");
        let now = i64::MAX / 4;
        let mut through_link = told(&link.join("t-9").display().to_string());
        through_link.started_ms = now;
        let ask = end_call_over(Vec::new(), vec![through_link.clone()]);
        let held = held_checkouts(&ask, &[]);
        assert_eq!(held.len(), 1, "{held:?}");
        let standing = real.join("t-9/crates").display().to_string();
        assert!(sweep::owner(&standing, &held).is_some(), "{held:?}");
        let tree = link.join("t-9/crates").display().to_string();
        let mut real_told = told(&real.join("t-9").display().to_string());
        real_told.started_ms = now;
        let ask = end_call_over(Vec::new(), vec![real_told]);
        assert!(held_checkouts(&ask, &[tree]).is_empty());
    }

    /// A folder deleted and made again under the same name is not the old worker's checkout
    /// (r3w-6): a process standing there is outside every checkout, which is the refusal.
    #[test]
    fn a_folder_made_again_under_a_workers_name_is_not_that_workers_checkout() {
        let dir = tempfile::tempdir().expect("scratch");
        let old = dir.path().join("t-9");
        let current = dir.path().join("t-8");
        std::fs::create_dir_all(&old).expect("folder");
        std::fs::create_dir_all(&current).expect("folder");
        let mut long_gone = told(&old.display().to_string());
        long_gone.started_ms = 1_000;
        let mut live = told(&current.display().to_string());
        live.started_ms = i64::MAX / 4;
        let ask = end_call_over(Vec::new(), vec![long_gone, live]);
        let held = held_checkouts(&ask, &[]);
        let paths: Vec<&str> = held.iter().map(|one| one.path.as_str()).collect();
        assert_eq!(paths.len(), 1, "{paths:?}");
        assert!(paths[0].ends_with("t-8"), "{paths:?}");
    }

    /// The window reads a live coordinator seat's tree from the pane it
    /// placed there; a seat with no team, no pane or no tree here adds
    /// nothing.
    #[test]
    fn a_coordinator_seats_tree_is_the_one_the_window_placed_it_in() {
        struct Placed;
        impl crate::agent_teams::Host for Placed {
            fn split(
                &self,
                _team: &str,
                _leader_term: u32,
                _from_term: u32,
                _pane: &str,
                _direction: zerocode_core::agent_teams::Direction,
                _command: &str,
                _token: &str,
            ) -> Option<u32> {
                None
            }
            fn send(&self, _term: u32, _text: &str) -> bool {
                false
            }
            fn capture(&self, _term: u32) -> Option<String> {
                None
            }
            fn focus(&self, _term: u32) -> bool {
                false
            }
            fn close(&self, _term: u32) {}
            fn worktree_of(&self, term: u32) -> Option<std::path::PathBuf> {
                (term == 78_914).then(|| std::path::PathBuf::from("/main-checkout"))
            }
        }
        let team = "team-t78914-sweep";
        crate::agent_teams::teams().insert(
            team.to_string(),
            zerocode_core::agent_teams::Team::new(team, "unused", 78_914),
        );
        let leader = zerocode_core::agent_teams::LEADER_PANE;
        let trees = coordinator_trees(
            &[
                format!("{team}/{leader}"),
                format!("{team}/%99"),
                format!("team-nobody/{leader}"),
                "no-slash".to_string(),
            ],
            &Placed,
        );
        crate::agent_teams::teams().remove(team);
        assert_eq!(trees, vec!["/main-checkout".to_string()]);
    }
}
