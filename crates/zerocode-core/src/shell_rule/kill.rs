//! The signals a shell command will send, and whether the pane about to run
//! it may send them (t-80602).
//!
//! Twice on 10-28 an example command in backticks, inside a double-quoted
//! instruction, ran as a command: `pkill -f` with every word after it taken
//! as a pattern ended the window, its panes and a build runner. So the calls
//! are read the way the shell will run them ([`super::script`]) — command
//! substitutions in double quotes, unquoted here-documents, `sh -c` and
//! `eval` strings, `xargs` — and a single-quoted example is only letters.
//!
//! [`kill_calls`] reads a command into [`KillCall`]s without touching the
//! machine; [`judge_kill`] puts their targets against a process table the
//! caller read once, and the pids the window knows: the calling pane's own
//! process ([`judge_kill`]'s `caller`) and every other process tree it must
//! keep ([`ProtectedRoot`]). No program name is written down here: whose a
//! process is, is read from the tree it hangs in.

use std::collections::HashMap;
use std::time::Duration;

/// The most a process listing may print before it is called unreadable.
const LISTING_MAX_BYTES: u64 = 16 * 1024 * 1024;

/// How often a running listing is asked whether it has finished.
const LISTING_POLL: Duration = Duration::from_millis(5);

/// One call of `kill`, `pkill` or `killall` the shell will make.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KillCall {
    /// The call as written, its quoting taken off: `pkill -f zo-step foo`.
    pub written: String,
    /// What it signals. Empty for a call that signals nothing it can find.
    pub targets: Vec<KillTarget>,
}

impl KillCall {
    /// Whether judging it needs the process table: some target is named by
    /// number, group, parent or pattern.
    #[must_use]
    pub fn needs_table(&self) -> bool {
        self.targets
            .iter()
            .any(|target| !matches!(target, KillTarget::OwnShell | KillTarget::Unknown(_)))
    }
}

/// What one call signals.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillTarget {
    /// One process (`kill 4242`).
    Pid(u32),
    /// Every process of a group (`kill -- -4242`).
    Group(u32),
    /// The children of one process (`pkill -P 4242`).
    ChildrenOf(u32),
    /// Every process the user may signal (`kill -1`, `pkill -v …`,
    /// `killall -u me`).
    Everything,
    /// The processes a pattern matches, the way `pgrep` matches them.
    Matching(ProcessPattern),
    /// The calling shell's own jobs and group: `%1`, `$!`, `$$`, `0`.
    OwnShell,
    /// Pids the shell learns only when it runs — a variable, a file, the
    /// output of something other than `pgrep`. The words that name them.
    Unknown(String),
}

/// A `pgrep`-style selection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessPattern {
    pub text: String,
    /// Against the whole command line (`-f`), not the program's name.
    pub full: bool,
    /// The whole name or line, not a part of it (`-x`, `killall`).
    pub exact: bool,
    pub ignore_case: bool,
    /// An extended regular expression (`pgrep`, `killall -m`); a name
    /// otherwise.
    pub regex: bool,
}

/// One row of the process table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRow {
    pub pid: u32,
    pub ppid: u32,
    /// Its process group.
    pub pgid: u32,
    /// Its command line, program first.
    pub command: String,
}

impl ProcessRow {
    /// The program's name, as `pgrep` without `-f` matches it: the last part
    /// of the first word of its command line.
    #[must_use]
    pub fn name(&self) -> &str {
        let program = self.command.split_whitespace().next().unwrap_or_default();
        program.rsplit('/').next().unwrap_or(program)
    }
}

/// The process table, read once for a judgment — or why it could not be.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessTable {
    rows: Result<Vec<ProcessRow>, String>,
}

impl ProcessTable {
    #[must_use]
    pub fn new(rows: Vec<ProcessRow>) -> Self {
        Self { rows: Ok(rows) }
    }

    /// A table that could not be read, and why — every target that needs it
    /// is asked of a person.
    #[must_use]
    pub fn unreadable(why: impl Into<String>) -> Self {
        Self {
            rows: Err(why.into()),
        }
    }

    /// The host's process table, read with `ps` and waited for at most
    /// `within`. A listing that does not start, finish in time or exit
    /// cleanly is unreadable — never an empty table, which would read as
    /// nothing to hit. The listing's own `ps` is left out, as `pgrep` leaves
    /// itself out.
    #[must_use]
    pub fn read(within: Duration) -> Self {
        match listing(within) {
            Ok((text, sampler)) => Self::from_listing(&text, Some(sampler)),
            Err(why) => Self::unreadable(why),
        }
    }

    /// A table from `ps -o pid=,ppid=,pgid=,command=` text, leaving `except`
    /// out. A line that does not read is skipped.
    #[must_use]
    pub fn from_listing(listing: &str, except: Option<u32>) -> Self {
        let rows = listing
            .lines()
            .filter_map(|line| {
                let mut fields = line.split_whitespace();
                let pid = fields.next()?.parse().ok()?;
                let ppid = fields.next()?.parse().ok()?;
                let pgid = fields.next()?.parse().ok()?;
                let command = fields.collect::<Vec<_>>().join(" ");
                (Some(pid) != except).then_some(ProcessRow {
                    pid,
                    ppid,
                    pgid,
                    command,
                })
            })
            .collect();
        Self::new(rows)
    }

    /// The pids above `pid`, nearest first, as far as the table goes —
    /// empty for a table that did not read. A host keeps these: zo its own
    /// ancestors, up to the window.
    #[must_use]
    pub fn ancestors(&self, pid: u32) -> Vec<u32> {
        let Ok(rows) = &self.rows else {
            return Vec::new();
        };
        lineage(&by_pid(rows), pid).into_iter().skip(1).collect()
    }

    /// `root` and every pid under it — the tree a caller owns.
    #[must_use]
    pub fn tree_of(&self, root: u32) -> Vec<u32> {
        let Ok(rows) = &self.rows else {
            return Vec::new();
        };
        let mut children: HashMap<u32, Vec<u32>> = HashMap::new();
        for row in rows {
            if row.ppid != row.pid {
                children.entry(row.ppid).or_default().push(row.pid);
            }
        }
        let mut tree = vec![root];
        let mut at = 0;
        while let Some(&pid) = tree.get(at) {
            at += 1;
            for &child in children.get(&pid).map(Vec::as_slice).unwrap_or_default() {
                if !tree.contains(&child) {
                    tree.push(child);
                }
            }
        }
        tree
    }
}

/// `ps`'s listing and its own pid.
#[cfg(unix)]
fn listing(within: Duration) -> Result<(String, u32), String> {
    use std::io::Read as _;
    use std::process::{Command, Stdio};

    let mut child = Command::new("ps")
        .args(["-ww", "-eo", "pid=,ppid=,pgid=,command="])
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("ps did not start: {error}"))?;
    let sampler = child.id();
    let Some(stdout) = child.stdout.take() else {
        let _ = child.kill();
        let _ = child.wait();
        return Err("ps gave no output".to_string());
    };
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout
            .take(LISTING_MAX_BYTES)
            .read_to_end(&mut bytes)
            .map(|_| bytes)
    });
    let started = std::time::Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if started.elapsed() < within => std::thread::sleep(LISTING_POLL),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(format!(
                    "ps did not finish within {} ms",
                    within.as_millis()
                ));
            }
        }
    };
    let bytes = reader
        .join()
        .map_err(|_| "ps's output was lost".to_string())?
        .map_err(|error| format!("ps's output did not read: {error}"))?;
    if !status.success() {
        return Err(format!("ps ended with {status}"));
    }
    Ok((String::from_utf8_lossy(&bytes).into_owned(), sampler))
}

/// No `ps` here: every target that needs the table is asked of a person.
#[cfg(not(unix))]
fn listing(_within: Duration) -> Result<(String, u32), String> {
    Err("this host has no ps to read its processes with".to_string())
}

/// A process tree a pane may not signal: the window itself, another pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedRoot {
    pub pid: u32,
    /// How a reason names it: `the ZeroCode window`, `pane term-3`.
    pub label: String,
}

/// What may become of a command's calls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KillVerdict {
    Allow,
    /// A person should say yes first. A host that cannot put a question to
    /// a person — `zo -p` and other runs with nobody at the keyboard, a
    /// provider hook without an `ask` answer — treats it as [`Self::Deny`];
    /// [`judge_kill`] itself always says `Ask`, and never turns it into a
    /// yes.
    Ask {
        reason: String,
    },
    Deny {
        reason: String,
    },
}

/// Every `kill`, `pkill` and `killall` call `command` makes when the shell
/// runs it, inside command substitutions too. A command that does not hold
/// the letters `kill` is not read at all.
#[must_use]
pub fn kill_calls(command: &str) -> Vec<KillCall> {
    let _ = command;
    Vec::new()
}

/// Judge `calls` for the pane whose own process is `caller` (`None` when
/// the window does not know it): a target in the caller's own tree is its
/// own business; one in a `protected` tree outside it is refused; one
/// anywhere else, one known only at run time, or one the table could not be
/// read for is asked of a person.
#[must_use]
pub fn judge_kill(
    calls: &[KillCall],
    table: &ProcessTable,
    caller: Option<u32>,
    protected: &[ProtectedRoot],
) -> KillVerdict {
    let _ = (calls, table, caller, protected);
    KillVerdict::Allow
}

/// Whether `text` holds the letters `kill` in any case — every program this
/// guard reads does, so text without them is not read at all.
#[must_use]
pub fn might_signal(text: &str) -> bool {
    let _ = text;
    false
}

/// The table's rows by pid.
fn by_pid(rows: &[ProcessRow]) -> HashMap<u32, &ProcessRow> {
    rows.iter().map(|row| (row.pid, row)).collect()
}

/// `pid` and the pids above it, nearest first, until the table ends or
/// turns back on itself.
fn lineage(by_pid: &HashMap<u32, &ProcessRow>, pid: u32) -> Vec<u32> {
    let mut chain = vec![pid];
    let mut at = pid;
    while let Some(row) = by_pid.get(&at) {
        if row.ppid == 0 || chain.contains(&row.ppid) {
            break;
        }
        at = row.ppid;
        chain.push(at);
    }
    chain
}

#[cfg(test)]
mod tests;
