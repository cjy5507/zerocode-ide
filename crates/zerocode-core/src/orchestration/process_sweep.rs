//! `process-sweep` (t-78914): the coordinator's one road to end a stray
//! process. It lists by a plain substring, then ends only the pids that list
//! showed, one at a time, each read again right before its signal.
//!
//! Twice on 2026-10-28 (03:23 and 21:2x) one pattern kill took a person's
//! processes, the window and its panes with it: BSD `pkill -f a -P 1` reads
//! every word after the pattern as one more pattern, and a backtick inside a
//! quoted pattern ran before `pkill` ever saw it. So this verb has no pattern
//! road, no "every child of" road and no "everything" road — a pid is ended
//! only when the caller names it together with the mark the list printed for
//! it, and the mark binds the pid to the start time and command line it had
//! when it was shown. A pid the system reused since is a different mark.
//!
//! Whether a row may be ended at all is not judged here. That judgment is the
//! blocking road's (t-80602, `shell_rule::kill::judge_kill`), and two copies of
//! one protection list would drift; so the judgment arrives through [`Judge`],
//! and the window decides which judge stands behind it.

use std::collections::HashMap;
use std::path::Path;

use sha2::{Digest, Sha256};

use super::{Ledger, Words};

/// The verb's one spelling: the table, the plan arm and the window's carrying
/// of it all say it.
pub const VERB: &str = "process-sweep";

/// The rule this verb exists for, said wherever the verb speaks.
pub const NEVER_BY_PATTERN: &str = "패턴으로 죽이는 명령은 쓰지 않는다 — 인자가 패턴이 된다 (pkill -f a b reads b as a second pattern)";

/// A needle shorter than this matches most of the table, which is a pattern
/// kill's blast radius wearing a list's clothes.
const NEEDLE_MIN_CHARS: usize = 2;
const NEEDLE_MAX_BYTES: usize = 200;
/// How many pids one call may end. Enough for one worktree's build; a call
/// that needs more is a call that should have looked first.
pub const TARGETS_MAX: usize = 32;
/// How many matching rows one list answers with; the count of the rest is said.
const ROWS_SHOWN_MAX: usize = 200;
/// Hex digits of the mark: 48 bits, so a reused pid matching by chance is not
/// a case anybody has to reason about.
const MARK_HEX: usize = 12;
/// How much of a command line the ledger's line carries per pid.
const LINE_ARGS_MAX: usize = 80;

/// One process-table row as this verb reads it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcessRow {
    pub pid: u32,
    pub ppid: u32,
    /// The system's start identity for the pid (`lstart` on Unix, creation
    /// ticks on Windows) — the half of the identity a pid alone does not carry.
    pub started: String,
    pub args: String,
    /// Where it stands, when the window could read that.
    pub cwd: Option<String>,
}

/// A worker checkout the ledger knows, for saying whose a process is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Checkout {
    pub path: String,
    pub run: String,
    pub worker: String,
    pub task: Option<String>,
}

/// One pid the caller asked to end, with the mark the list showed for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub pid: u32,
    pub mark: String,
}

/// What the ledger decided and the window carries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProcessSweepAsk {
    /// List the rows whose command line holds `needle`, and change nothing.
    Find {
        needle: String,
        checkouts: Vec<Checkout>,
    },
    /// End these pids, one at a time, and tell `run`'s coordinator what was done.
    End { run: String, targets: Vec<Target> },
}

/// Whether one row may be ended. The shape the blocking road's `judge_kill`
/// answers, without its `Ask`: this verb has nobody to ask mid-call, so a
/// judge that would ask answers [`Judgment::Deny`] with its question as `why`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Judgment {
    Allow,
    Deny { why: String },
}

/// The protection judgment, from outside. The window's judge holds who is
/// asking and what is protected; this module only asks it, per row, with the
/// whole table beside the row so ancestry can be read.
pub trait Judge {
    fn judge(&self, row: &ProcessRow, table: &[ProcessRow]) -> Judgment;
}

/// The two things the end road needs from the system, from outside, so a test
/// never reaches a real process.
pub trait Signals {
    /// `(started, args)` for `pid` as the system reads it NOW, or `None` when
    /// no such process stands.
    fn reread(&self, pid: u32) -> Option<(String, String)>;
    /// Ask exactly this one pid to end (TERM on Unix). Never a group or a tree.
    fn terminate(&self, pid: u32) -> Result<(), String>;
}

/// What became of one asked pid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Ended,
    /// No process stands under the pid any more.
    Gone,
    /// The pid stands, but not as the process the list showed: a different
    /// start or command line, so it is a different process and is left alone.
    Changed,
    Refused {
        why: String,
    },
    Failed {
        why: String,
    },
}

impl Outcome {
    const fn word(&self) -> &'static str {
        match self {
            Self::Ended => "ended",
            Self::Gone => "gone",
            Self::Changed => "changed",
            Self::Refused { .. } => "refused",
            Self::Failed { .. } => "failed",
        }
    }

    fn why(&self) -> Option<&str> {
        match self {
            Self::Refused { why } | Self::Failed { why } => Some(why.as_str()),
            Self::Changed => Some(
                "the pid now carries a different start or command line than the list showed — \
                 list again",
            ),
            Self::Ended | Self::Gone => None,
        }
    }
}

/// One asked pid and what became of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ended {
    pub target: Target,
    /// The command line the pid had when it was judged, masked; empty when it
    /// was gone before it could be read.
    pub args: String,
    pub outcome: Outcome,
}

/// The mark of one row: what the list prints beside its pid, and what the end
/// road demands back. It covers the pid, its start and its command line, so a
/// pid reused by another process carries another mark.
#[must_use]
pub fn mark(pid: u32, started: &str, args: &str) -> String {
    let mut hash = Sha256::new();
    hash.update(pid.to_string().as_bytes());
    hash.update([0]);
    hash.update(started.as_bytes());
    hash.update([0]);
    hash.update(args.as_bytes());
    let digest = hash.finalize();
    let mut hex = String::with_capacity(MARK_HEX);
    for byte in digest.iter().take(MARK_HEX / 2) {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn row_mark(row: &ProcessRow) -> String {
    mark(row.pid, &row.started, &row.args)
}

fn target_word(pid: u32, mark: &str) -> String {
    format!("{pid}@{mark}")
}

/// A command line as it may be said: private values masked.
fn said_args(args: &str) -> String {
    crate::private_data::mask(args).text
}

/// Decide one call: which mode, and with what.
///
/// `run` is asked only by the end road — a list changes nothing and any pane
/// may ask for one; ending is the coordinator's, from the run it is bound to.
pub(super) fn ask(
    ledger: &Ledger,
    words: &Words,
    team: &str,
    pane: &str,
    run: impl FnOnce() -> Result<String, String>,
) -> Result<ProcessSweepAsk, String> {
    if !words.positional.is_empty() {
        return Err(format!(
            "{VERB} takes no bare words — list with --find <text>, then end with --kill \
             <pid@mark>[,<pid@mark>…]. {NEVER_BY_PATTERN}"
        ));
    }
    match (words.value("--find"), words.value("--kill")) {
        (Some(_), Some(_)) => Err(format!(
            "{VERB} lists or ends in one call, never both — list with --find first, then end what \
             it showed with --kill"
        )),
        (None, None) => Err(format!(
            "{VERB} needs --find <text> (list, changes nothing) or --kill <pid@mark>[,…] (end \
             what a list showed). {NEVER_BY_PATTERN}"
        )),
        (Some(needle), None) => Ok(ProcessSweepAsk::Find {
            needle: needle_of(needle)?,
            checkouts: checkouts(ledger),
        }),
        (None, Some(_)) => {
            let run_id = run()?;
            let held = ledger
                .run(&run_id)
                .ok_or_else(|| super::unknown_run(&run_id))?;
            if let Some(worker) = held.worker_in_pane(team, pane) {
                return Err(format!(
                    "{VERB} --kill is the coordinator's, and this pane is worker {} — say in your \
                     report which pids you want ended and let the coordinator end them",
                    worker.id
                ));
            }
            Ok(ProcessSweepAsk::End {
                run: run_id,
                targets: targets_of(&words.list("--kill"))?,
            })
        }
    }
}

fn needle_of(needle: &str) -> Result<String, String> {
    if needle.chars().any(char::is_control) {
        return Err("--find is plain text, and it holds a control character".to_string());
    }
    let trimmed = needle.trim();
    if trimmed.chars().count() < NEEDLE_MIN_CHARS {
        return Err(format!(
            "--find needs at least {NEEDLE_MIN_CHARS} characters — a shorter text matches most \
             of the table"
        ));
    }
    if trimmed.len() > NEEDLE_MAX_BYTES {
        return Err(format!("--find is at most {NEEDLE_MAX_BYTES} bytes"));
    }
    Ok(trimmed.to_string())
}

fn targets_of(words: &[String]) -> Result<Vec<Target>, String> {
    if words.is_empty() {
        return Err("--kill names at least one pid@mark from a list".to_string());
    }
    if words.len() > TARGETS_MAX {
        return Err(format!(
            "--kill names at most {TARGETS_MAX} pids in one call, and this names {}",
            words.len()
        ));
    }
    let mut targets: Vec<Target> = Vec::with_capacity(words.len());
    for word in words {
        let Some((pid, mark)) = word.split_once('@') else {
            return Err(format!(
                "--kill takes pid@mark exactly as --find printed it, and {word:?} has no mark — \
                 a bare pid could be any process by now"
            ));
        };
        let pid = pid
            .parse::<u32>()
            .ok()
            .filter(|pid| *pid > 1)
            .ok_or_else(|| format!("{word:?} does not start with a process id above 1"))?;
        let well_formed = mark.len() == MARK_HEX
            && mark
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
        if !well_formed {
            return Err(format!(
                "{word:?} carries no mark --find prints ({MARK_HEX} lowercase hex digits)"
            ));
        }
        if targets.iter().any(|held| held.pid == pid) {
            return Err(format!("--kill names pid {pid} twice"));
        }
        targets.push(Target {
            pid,
            mark: mark.to_string(),
        });
    }
    Ok(targets)
}

/// Every worker checkout the ledger was told, across its runs.
fn checkouts(ledger: &Ledger) -> Vec<Checkout> {
    let mut found = Vec::new();
    for run in ledger.runs() {
        for worker in &run.workers {
            let Some(path) = worker.checkout.clone() else {
                continue;
            };
            let task = worker
                .dispatch
                .as_deref()
                .and_then(|dispatch| run.dispatch(dispatch))
                .map(|dispatch| dispatch.task.clone());
            found.push(Checkout {
                path,
                run: run.id.clone(),
                worker: worker.id.clone(),
                task,
            });
        }
    }
    found
}

/// Whose checkout `cwd` stands in: the deepest one that holds it, and of two
/// workers told the same path, the later.
fn owner<'a>(cwd: &str, checkouts: &'a [Checkout]) -> Option<&'a Checkout> {
    let standing = Path::new(cwd);
    let mut best: Option<&Checkout> = None;
    for checkout in checkouts {
        if !standing.starts_with(&checkout.path) {
            continue;
        }
        if best.is_none_or(|held| checkout.path.len() >= held.path.len()) {
            best = Some(checkout);
        }
    }
    best
}

/// The asking command's own rows hold the needle too (`zerocode-orc
/// process-sweep --find cargo` contains `cargo`); they are not candidates.
fn is_candidate(row: &ProcessRow, needle: &str) -> bool {
    row.args.contains(needle) && !row.args.contains(VERB)
}

/// The pids a list for `needle` will show, in pid order — so the window reads
/// where those, and only those, stand.
#[must_use]
pub fn matching_pids(table: &[ProcessRow], needle: &str) -> Vec<u32> {
    let mut pids: Vec<u32> = table
        .iter()
        .filter(|row| is_candidate(row, needle))
        .map(|row| row.pid)
        .collect();
    pids.sort_unstable();
    pids.truncate(ROWS_SHOWN_MAX);
    pids
}

/// The list: every row whose command line holds `needle` as plain text, with
/// its mark, parent, start, place, owner and the judge's word. Nothing is
/// signalled.
#[must_use]
pub fn find(
    table: &[ProcessRow],
    needle: &str,
    checkouts: &[Checkout],
    judge: &dyn Judge,
) -> serde_json::Value {
    let mut matched: Vec<&ProcessRow> = table
        .iter()
        .filter(|row| is_candidate(row, needle))
        .collect();
    matched.sort_by_key(|row| row.pid);
    let total = matched.len();
    let rows: Vec<serde_json::Value> = matched
        .into_iter()
        .take(ROWS_SHOWN_MAX)
        .map(|row| {
            let owner = row
                .cwd
                .as_deref()
                .and_then(|cwd| owner(cwd, checkouts))
                .map(|held| {
                    serde_json::json!({
                        "run": held.run,
                        "worker": held.worker,
                        "task": held.task,
                        "checkout": held.path,
                    })
                });
            let (verdict, why) = match judge.judge(row, table) {
                Judgment::Allow => ("may_end", None),
                Judgment::Deny { why } => ("protected", Some(why)),
            };
            let mut said = serde_json::json!({
                "target": target_word(row.pid, &row_mark(row)),
                "pid": row.pid,
                "ppid": row.ppid,
                "started": row.started,
                "cwd": row.cwd,
                "owner": owner,
                "verdict": verdict,
                "args": said_args(&row.args),
            });
            if let Some(why) = why {
                said["why"] = why.into();
            }
            said
        })
        .collect();
    serde_json::json!({
        "needle": needle,
        "matched": total,
        "shown": rows.len(),
        "rows": rows,
        "next": format!(
            "zerocode-orc {VERB} --kill <target>[,<target>…] --retry-request <name> — the \
             target column exactly; each pid is read again before its signal"
        ),
        "never": NEVER_BY_PATTERN,
    })
}

/// End the asked pids in order, one at a time.
///
/// Each one is found in `table` (read once for the whole call, so the judge
/// can see ancestry), its mark compared, judged, then read AGAIN through
/// `signals` immediately before its signal — the list may be minutes old, and
/// the pid may have been handed to another process meanwhile.
pub fn end(
    table: &[ProcessRow],
    targets: &[Target],
    judge: &dyn Judge,
    signals: &dyn Signals,
) -> Vec<Ended> {
    targets
        .iter()
        .map(|target| {
            let (args, outcome) = end_one(table, target, judge, signals);
            Ended {
                target: target.clone(),
                args,
                outcome,
            }
        })
        .collect()
}

fn end_one(
    table: &[ProcessRow],
    target: &Target,
    judge: &dyn Judge,
    signals: &dyn Signals,
) -> (String, Outcome) {
    let Some(row) = table.iter().find(|row| row.pid == target.pid) else {
        return (String::new(), Outcome::Gone);
    };
    let args = said_args(&row.args);
    if row_mark(row) != target.mark {
        return (args, Outcome::Changed);
    }
    if let Judgment::Deny { why } = judge.judge(row, table) {
        return (args, Outcome::Refused { why });
    }
    let Some((started, now)) = signals.reread(target.pid) else {
        return (args, Outcome::Gone);
    };
    if mark(target.pid, &started, &now) != target.mark {
        return (said_args(&now), Outcome::Changed);
    }
    match signals.terminate(target.pid) {
        Ok(()) => (args, Outcome::Ended),
        Err(why) => (args, Outcome::Failed { why }),
    }
}

/// The verb's answer to an end call.
#[must_use]
pub fn end_answer(run: &str, ended: &[Ended]) -> serde_json::Value {
    let rows: Vec<serde_json::Value> = ended
        .iter()
        .map(|one| {
            let mut said = serde_json::json!({
                "target": target_word(one.target.pid, &one.target.mark),
                "pid": one.target.pid,
                "outcome": one.outcome.word(),
                "args": one.args,
            });
            if let Some(why) = one.outcome.why() {
                said["why"] = why.into();
            }
            said
        })
        .collect();
    serde_json::json!({
        "run": run,
        "asked": ended.len(),
        "ended": ended.iter().filter(|one| one.outcome == Outcome::Ended).count(),
        "rows": rows,
        "never": NEVER_BY_PATTERN,
    })
}

/// The one line the ledger keeps of an end call: how many ended, and each pid
/// with its outcome and the start of its command line.
#[must_use]
pub fn ledger_line(ended: &[Ended]) -> String {
    let count = ended
        .iter()
        .filter(|one| one.outcome == Outcome::Ended)
        .count();
    let each: Vec<String> = ended
        .iter()
        .map(|one| {
            let mut piece = format!("{} {}", one.target.pid, one.outcome.word());
            if !one.args.is_empty() {
                let args: String = one.args.chars().take(LINE_ARGS_MAX).collect();
                let cut = if one.args.chars().count() > LINE_ARGS_MAX {
                    "…"
                } else {
                    ""
                };
                piece.push_str(&format!(" ({args}{cut})"));
            }
            if let Outcome::Refused { why } | Outcome::Failed { why } = &one.outcome {
                piece.push_str(&format!(": {why}"));
            }
            piece
        })
        .collect();
    format!(
        "{VERB}: ended {count} of {} — {}",
        ended.len(),
        each.join("; ")
    )
}

/// Every pid the end call asked about, in a receipt-sized word, so one call's
/// ledger line is filed once however often the window is asked to file it.
#[must_use]
pub fn receipt_of(run: &str, targets: &[Target], now_ms: i64) -> String {
    let words: Vec<String> = targets
        .iter()
        .map(|target| target_word(target.pid, &target.mark))
        .collect();
    format!("{VERB}:{run}:{now_ms}:{}", words.join(","))
}

/// Where each listed process stands (`cwds`, by pid), folded into its row.
pub fn with_cwds(table: &mut [ProcessRow], cwds: &HashMap<u32, String>) {
    for row in table.iter_mut() {
        if let Some(cwd) = cwds.get(&row.pid) {
            row.cwd = Some(cwd.clone());
        }
    }
}
