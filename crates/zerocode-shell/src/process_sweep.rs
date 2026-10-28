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
/// ([`coordinator_trees`]).
pub(crate) fn carry(
    ask: &ProcessSweepAsk,
    roots: Option<KillRoots>,
    coordinator_trees: &[String],
) -> Result<Carried, String> {
    let (rows, table) = table()?;
    let checkouts = held_checkouts(ask, coordinator_trees);
    let judge = judge(roots, table, checkouts.clone(), std::process::id());
    carry_with(
        ask,
        &checkouts,
        rows,
        &crate::system_runtime::process_cwds,
        judge.as_ref(),
        &System::new(),
    )
}

/// The worker checkouts this call reads: the ask's, less each that is or
/// holds a coordinator's tree.
fn held_checkouts(ask: &ProcessSweepAsk, coordinator_trees: &[String]) -> Vec<Checkout> {
    sweep::outside(ask.checkouts().to_vec(), coordinator_trees)
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
fn judge(
    roots: Option<KillRoots>,
    table: ProcessTable,
    checkouts: Vec<Checkout>,
    window: u32,
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
            })
        }
        None => Box::new(NoRoots),
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
struct KillRule {
    /// The same read the rows came from, in the guard's shape.
    table: ProcessTable,
    caller: Option<u32>,
    protected: Vec<kill::ProtectedRoot>,
    checkouts: Vec<Checkout>,
}

impl KillRule {
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
        let call = KillCall {
            written: format!("kill {}", row.pid),
            targets: vec![KillTarget::Pid(row.pid)],
        };
        match kill::judge_kill(&[call], &self.table, self.caller, &self.protected) {
            KillVerdict::Allow => Judgment::Allow,
            KillVerdict::Deny { reason } => Judgment::Deny { why: reason },
            // For one pid the guard asks for two things: a process no pane
            // and not the window holds, or a table that did not read. Only
            // the first is the coordinator's to answer, and only for an
            // orphaned build; an unread table has no ancestry to follow, so
            // it stays a refusal.
            KillVerdict::Ask { reason } => match self.orphaned_in(row, rows) {
                Some(_) => Judgment::Allow,
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

struct NoRoots;

impl Judge for NoRoots {
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
            let cwds = read_cwds(&judge.places_for(&sweep::matching_pids(&table, needle)));
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
struct System {
    began: std::time::Instant,
}

impl System {
    fn new() -> Self {
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
    use std::cell::RefCell;
    use zerocode_core::orchestration::process_sweep::{Target, mark};

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
        let judge = judge(None, ProcessTable::new(Vec::new()), Vec::new(), 500);
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
            .map(|(pid, cwd)| (pid, cwd.to_string()))
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
        let judge = judge(Some(guarded_roots()), guard_table, checkouts.clone(), 500);
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
        ["/w/t-9", "/w/t-8"].into_iter().map(told).collect()
    }

    /// A worker checkout at `path`, as the ledger tells it.
    fn told(path: &str) -> Checkout {
        let at = path.rsplit('-').next().unwrap_or_default();
        Checkout {
            path: path.to_string(),
            run: "run-1".to_string(),
            worker: format!("w-{at}"),
            task: Some(format!("t-{at}")),
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
        let judge = judge(Some(guarded_roots()), table, guarded_checkouts(), 500);
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

    /// A process no pane holds that stands in no worker checkout — a
    /// build-line runner — is a question nobody can be asked: refused.
    #[test]
    fn an_orphan_outside_every_checkout_is_refused_as_unaskable() {
        let (signalled, rows) = guarded_end(&[4400]);
        assert!(signalled.is_empty(), "{signalled:?}");
        assert_eq!(rows[0]["outcome"], "refused");
        assert!(why(&rows[0]).starts_with("물을 수 없음"), "{rows:?}");
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
        told_rows.push(told("/build-line"));
        for tree in ["/build-line", "/build-line/crates/zerocode-shell"] {
            let (signalled, rows) = guarded_end_told(
                LISTING,
                ProcessTable::from_listing(LISTING, None),
                told_rows.clone(),
                &[tree.to_string()],
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
        told_rows.push(told("/build-line"));
        let ask = end_call_over(Vec::new(), told_rows);
        let held = held_checkouts(&ask, &["/w/coordinator".to_string()]);
        assert!(held.iter().any(|one| one.path == "/build-line"), "{held:?}");
        let held = held_checkouts(&ask, &["/build-line".to_string()]);
        let paths: Vec<&str> = held.iter().map(|one| one.path.as_str()).collect();
        assert_eq!(paths, vec!["/w/t-9", "/w/t-8"]);
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
