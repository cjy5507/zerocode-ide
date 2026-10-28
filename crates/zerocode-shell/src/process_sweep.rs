//! The window's half of `process-sweep` (t-78914): the process table, where
//! each listed process stands, the protection judgment and the one signal.
//!
//! The ledger decided the mode, the needle or the pids and the run
//! (`zerocode_core::orchestration::process_sweep`); this reads the system and
//! carries it out. The table is the status bar's own read
//! ([`crate::resource_usage::enumerate_processes`]) and the places are the
//! port list's ([`crate::system_runtime::process_cwds`]), so there is one
//! reading of each and nothing here to disagree with them.

use std::collections::HashMap;

use zerocode_core::orchestration::process_sweep::{
    self as sweep, Judge, Judgment, ProcessRow, ProcessSweepAsk, Signals,
};

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

/// Carry one decided call against this machine.
pub(crate) fn carry(ask: &ProcessSweepAsk, now_ms: i64) -> Result<Carried, String> {
    let judge = judge();
    carry_with(
        ask,
        &table,
        &crate::system_runtime::process_cwds,
        judge.as_ref(),
        &System,
        now_ms,
    )
}

/// The protection judgment this window stands behind the verb — the ONE place
/// it is chosen. The judgment itself belongs to the blocking road (t-80602,
/// `zerocode_core::shell_rule::kill::judge_kill`, which answers for the
/// window body and its helpers, the ledger's panes and worker CLIs, the build
/// line's runners, the caller's own ancestors, a person's tabs and the
/// system's processes); until that road is joined here, every row is
/// protected, so the verb lists and ends nothing.
fn judge() -> Box<dyn Judge> {
    Box::new(NotJoined)
}

struct NotJoined;

impl Judge for NotJoined {
    fn judge(&self, _row: &ProcessRow, _table: &[ProcessRow]) -> Judgment {
        Judgment::Deny {
            why: "the protection judgment is not joined to this window yet (t-80602 \
                  judge_kill), so nothing is ended"
                .to_string(),
        }
    }
}

fn carry_with(
    ask: &ProcessSweepAsk,
    read_table: &dyn Fn() -> Result<Vec<ProcessRow>, String>,
    read_cwds: &dyn Fn(&[u32]) -> HashMap<u32, String>,
    judge: &dyn Judge,
    signals: &dyn Signals,
    now_ms: i64,
) -> Result<Carried, String> {
    let mut table = read_table()?;
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

/// The table, every row that carries both halves of its identity. An empty
/// read is a failed read — this process itself is always in the table — and
/// nothing is listed or ended on it.
fn table() -> Result<Vec<ProcessRow>, String> {
    let sample = crate::resource_usage::enumerate_processes()?;
    let rows: Vec<ProcessRow> = sample
        .identified()
        .map(|(pid, ppid, started, args)| ProcessRow {
            pid,
            ppid,
            started: started.to_string(),
            args: args.to_string(),
            cwd: None,
        })
        .collect();
    if rows.is_empty() {
        return Err("the process table read empty, so nothing is listed or ended".to_string());
    }
    Ok(rows)
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

    /// A list asks where only its own matches stand, and says the place.
    #[test]
    fn a_list_reads_the_places_of_its_matches_only() {
        let fixture = vec![
            row(4100, "cargo build -p zerocode-shell"),
            row(4200, "node server.js"),
        ];
        let asked = RefCell::new(Vec::new());
        let read_table = || -> Result<Vec<ProcessRow>, String> { Ok(fixture.clone()) };
        let read_cwds = |pids: &[u32]| {
            asked.borrow_mut().extend_from_slice(pids);
            HashMap::from([(4100, "/w/t-1".to_string())])
        };
        let signals = Recorded {
            table: fixture.clone(),
            ended: RefCell::new(Vec::new()),
        };
        let carried = carry_with(
            &ProcessSweepAsk::Find {
                needle: "cargo".to_string(),
                checkouts: Vec::new(),
            },
            &read_table,
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
        let fixture = vec![row(4100, "cargo build -p zerocode-shell")];
        let read_table = || -> Result<Vec<ProcessRow>, String> { Ok(fixture.clone()) };
        let read_cwds = |_: &[u32]| HashMap::<u32, String>::new();
        let signals = Recorded {
            table: fixture.clone(),
            ended: RefCell::new(Vec::new()),
        };
        let targets = vec![Target {
            pid: 4100,
            mark: mark(4100, STARTED, "cargo build -p zerocode-shell"),
        }];
        let carried = carry_with(
            &ProcessSweepAsk::End {
                run: "run-1".to_string(),
                targets,
            },
            &read_table,
            &read_cwds,
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

    /// Until the blocking road's judgment is joined, the window's own judge
    /// protects every row: a list says so and an end call ends nothing.
    #[test]
    fn the_window_ends_nothing_until_the_judgment_is_joined() {
        let fixture = vec![row(4100, "cargo build")];
        let read_table = || -> Result<Vec<ProcessRow>, String> { Ok(fixture.clone()) };
        let read_cwds = |_: &[u32]| HashMap::<u32, String>::new();
        let signals = Recorded {
            table: fixture.clone(),
            ended: RefCell::new(Vec::new()),
        };
        let carried = carry_with(
            &ProcessSweepAsk::End {
                run: "run-1".to_string(),
                targets: vec![Target {
                    pid: 4100,
                    mark: mark(4100, STARTED, "cargo build"),
                }],
            },
            &read_table,
            &read_cwds,
            judge().as_ref(),
            &signals,
            7,
        )
        .expect("an end call");
        assert!(signals.ended.borrow().is_empty());
        let said: serde_json::Value = serde_json::from_str(&carried.said).expect("JSON");
        assert_eq!(said["rows"][0]["outcome"], "refused");
        assert!(
            said["rows"][0]["why"]
                .as_str()
                .is_some_and(|why| why.contains("t-80602")),
            "{said}"
        );
    }

    /// A table that could not be read is a refusal, never an empty list.
    #[test]
    fn an_unread_table_is_refused() {
        let read_table = || -> Result<Vec<ProcessRow>, String> { Err("ps timed out".to_string()) };
        let read_cwds = |_: &[u32]| HashMap::<u32, String>::new();
        let signals = Recorded {
            table: Vec::new(),
            ended: RefCell::new(Vec::new()),
        };
        let carried = carry_with(
            &ProcessSweepAsk::Find {
                needle: "cargo".to_string(),
                checkouts: Vec::new(),
            },
            &read_table,
            &read_cwds,
            &Allow,
            &signals,
            1,
        );
        assert!(carried.is_err());
    }
}
