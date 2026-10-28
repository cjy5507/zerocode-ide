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
    self as sweep, Judge, Judgment, ProcessRow, ProcessSweepAsk, Signals,
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
/// own process and the trees it may not signal, as the window reads them.
pub(crate) fn carry(
    ask: &ProcessSweepAsk,
    roots: Option<KillRoots>,
    now_ms: i64,
) -> Result<Carried, String> {
    let (rows, table) = table()?;
    let judge = judge(roots, table);
    carry_with(
        ask,
        rows,
        &crate::system_runtime::process_cwds,
        judge.as_ref(),
        &System,
        now_ms,
    )
}

/// The protection judgment this window stands behind the verb — the ONE place
/// it is chosen. A window that cannot say whose processes are whose ends
/// nothing.
fn judge(roots: Option<KillRoots>, table: ProcessTable) -> Box<dyn Judge> {
    match roots {
        Some(roots) => Box::new(KillRule {
            table,
            caller: roots.caller,
            protected: roots.protected,
        }),
        None => Box::new(NoRoots),
    }
}

/// The kill guard's rule, asked of one pid at a time as a `kill <pid>` would
/// be: a process in the asking pane's own tree may end; one in the window's
/// or another pane's tree may not; anything else is a question for a person —
/// and this verb puts no question to anybody mid-call, so a question is a
/// refusal that says so.
struct KillRule {
    /// The same read the rows came from, in the guard's shape.
    table: ProcessTable,
    caller: Option<u32>,
    protected: Vec<kill::ProtectedRoot>,
}

impl Judge for KillRule {
    /// `_table` is the same read as `self.table`; the guard's rule reads its
    /// own shape of it.
    fn judge(&self, row: &ProcessRow, _table: &[ProcessRow]) -> Judgment {
        let call = KillCall {
            written: format!("kill {}", row.pid),
            targets: vec![KillTarget::Pid(row.pid)],
        };
        match kill::judge_kill(&[call], &self.table, self.caller, &self.protected) {
            KillVerdict::Allow => Judgment::Allow,
            KillVerdict::Deny { reason } => Judgment::Deny { why: reason },
            KillVerdict::Ask { reason } => Judgment::Deny {
                why: format!(
                    "물을 수 없음 — {} asks nobody mid-call, so what a person would be asked \
                     about is refused: {reason}",
                    sweep::VERB
                ),
            },
        }
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

fn carry_with(
    ask: &ProcessSweepAsk,
    mut table: Vec<ProcessRow>,
    read_cwds: &dyn Fn(&[u32]) -> HashMap<u32, String>,
    judge: &dyn Judge,
    signals: &dyn Signals,
    now_ms: i64,
) -> Result<Carried, String> {
    match ask {
        ProcessSweepAsk::Find { needle, checkouts } => {
            let cwds = read_cwds(&sweep::matching_pids(&table, needle));
            sweep::with_cwds(&mut table, &cwds);
            Ok(Carried {
                said: sweep::find(&table, needle, checkouts, judge).to_string(),
                filed: None,
            })
        }
        ProcessSweepAsk::End { run, targets } => {
            let ended = sweep::end(&table, targets, judge, signals);
            Ok(Carried {
                said: sweep::end_answer(run, &ended).to_string(),
                filed: Some(Filed {
                    run: run.clone(),
                    line: sweep::ledger_line(&ended),
                    receipt: sweep::receipt_of(run, targets, now_ms),
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

struct System;

impl Signals for System {
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
    /// pid it was asked to end instead of signalling anything.
    struct Recorded {
        table: Vec<ProcessRow>,
        ended: RefCell<Vec<u32>>,
    }

    impl Recorded {
        fn over(table: &[ProcessRow]) -> Self {
            Self {
                table: table.to_vec(),
                ended: RefCell::new(Vec::new()),
            }
        }
    }

    impl Signals for Recorded {
        fn reread(&self, pid: u32) -> Option<(String, String)> {
            self.table
                .iter()
                .find(|row| row.pid == pid)
                .map(|row| (row.started.clone(), row.args.clone()))
        }

        fn terminate(&self, pid: u32) -> Result<(), String> {
            self.ended.borrow_mut().push(pid);
            Ok(())
        }
    }

    fn end_call(targets: Vec<Target>) -> ProcessSweepAsk {
        ProcessSweepAsk::End {
            run: "run-1".to_string(),
            targets,
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
            },
            fixture,
            &read_cwds,
            &Allow,
            &signals,
            1,
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
            fixture,
            &no_cwds,
            &Allow,
            &signals,
            7,
        )
        .expect("an end call");
        assert_eq!(*signals.ended.borrow(), vec![4100]);
        let filed = carried.filed.expect("a line for the ledger");
        assert_eq!(filed.run, "run-1");
        assert!(
            filed.line.starts_with("process-sweep: ended 1 of 1"),
            "{}",
            filed.line
        );
        assert!(filed.receipt.contains("run-1"), "{}", filed.receipt);
    }

    /// A window that cannot say whose processes are whose ends nothing.
    #[test]
    fn without_the_panes_roots_nothing_is_ended() {
        let fixture = vec![row(4100, "cargo build")];
        let signals = Recorded::over(&fixture);
        let judge = judge(None, ProcessTable::new(Vec::new()));
        let carried = carry_with(
            &end_call(vec![target(4100, "cargo build")]),
            fixture,
            &no_cwds,
            judge.as_ref(),
            &signals,
            7,
        )
        .expect("an end call");
        assert!(signals.ended.borrow().is_empty());
        let said: serde_json::Value = serde_json::from_str(&carried.said).expect("JSON");
        assert_eq!(said["rows"][0]["outcome"], "refused");
    }

    /// Through the kill guard's real rule over one fixture table (`ps -o
    /// pid=,ppid=,pgid=,command=` text): the asking pane's own build ends;
    /// another pane's build and the window are refused by name; an orphan
    /// nobody's pane holds would be a question for a person, so it is refused
    /// as one that cannot be asked. Nothing real is signalled.
    #[test]
    fn the_kill_guards_rule_decides_each_pid() {
        let listing = "\
            500 1 500 /Applications/ZeroCode.app/Contents/MacOS/zerocode-shell\n\
            600 500 600 -zsh\n\
            700 600 600 claude --model opus\n\
            4100 700 4100 cargo build -p zerocode-shell\n\
            610 500 610 -zsh\n\
            4200 610 4200 cargo test -p zerocode-core\n\
            4300 1 4300 cargo build -p orphaned\n";
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
        let roots = KillRoots {
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
        };
        let judge = judge(Some(roots), ProcessTable::from_listing(listing, None));
        let signals = Recorded::over(&rows);
        let targets: Vec<Target> = [500, 4100, 4200, 4300]
            .into_iter()
            .map(|pid| {
                let held = rows.iter().find(|row| row.pid == pid).expect("a row");
                target(pid, &held.args)
            })
            .collect();
        let carried = carry_with(
            &end_call(targets),
            rows,
            &no_cwds,
            judge.as_ref(),
            &signals,
            7,
        )
        .expect("an end call");
        assert_eq!(*signals.ended.borrow(), vec![4100], "only the own build");
        let said: serde_json::Value = serde_json::from_str(&carried.said).expect("JSON");
        let why = |at: usize| {
            said["rows"][at]["why"]
                .as_str()
                .unwrap_or_default()
                .to_string()
        };
        assert_eq!(said["rows"][0]["outcome"], "refused");
        assert!(why(0).contains("the ZeroCode window"), "{said}");
        assert_eq!(said["rows"][1]["outcome"], "ended");
        assert_eq!(said["rows"][2]["outcome"], "refused");
        assert!(why(2).contains("pane term-3"), "{said}");
        assert_eq!(said["rows"][3]["outcome"], "refused");
        assert!(why(3).starts_with("물을 수 없음"), "{said}");
    }
}
