//! The signals a shell command will send, and whether the pane about to run
//! it may send them (t-80602).
//!
//! Twice on 10-28 an example command in backticks, inside a double-quoted
//! instruction, ran as a command: `pkill -f` with every word after it taken
//! as a pattern ended the window, its panes and a build runner. So the calls
//! are read the way the shell will run them (`shell_rule::script`) — command
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

use super::SHELL_COMMANDS;
use super::parse::WRAPPER_COMMANDS;
use super::script::{self, Command, Script, Word};

/// How many times an `sh -c` or `eval` string is read inside another before
/// the rest is called unreadable. Chosen, not measured: the incident nested
/// none, a hand-written script rarely goes past two (`ssh … "bash -c '…'"`),
/// and each level is read again in full, so the bound keeps a hostile string
/// from costing the hook its 800 ms.
const MAX_STRING_DEPTH: usize = 8;

/// The most of a call or a process's name a reason quotes: two 80-column
/// lines, so the agent's screen shows the reason without the command
/// burying it. The incident's whole call, `pkill -f zo-step foo`, is 20.
const QUOTED_CHARS: usize = 160;

/// The variable a ZeroCode window puts in every pane's environment: its own
/// pid. A host inside a pane (zo) reads it to know which tree is the
/// window's ([`ProtectedRoot::from_window_env`]).
pub const WINDOW_PID_ENV: &str = "ZEROCODE_WINDOW_PID";

/// How long a host waits for [`ProcessTable::read`] before it asks a person
/// instead: 800 ms, the window's hook reply budget (1,500 ms) less its
/// connect (500 ms) and reply margin (200 ms) — `zerocode_hookd`'s
/// `TURN_BRIEF_WALL`, which a test there holds equal to this.
pub const TABLE_READ_WALL: Duration = Duration::from_millis(800);

/// How a reason names the window itself.
const WINDOW_LABEL: &str = "the ZeroCode window";

/// Wrappers that run the command after them and are not in
/// [`WRAPPER_COMMANDS`], which the intent classifier reads.
const RUNS_THE_NEXT_COMMAND: &[&str] = &["sudo", "command", "exec", "builtin"];

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
    /// The program's name, as `pgrep` without `-f` and `killall` match it:
    /// the last part of the first word of its command line, without the `-`
    /// a login shell's opens with (`-zsh` is named `zsh`).
    #[must_use]
    pub fn name(&self) -> &str {
        let program = self.command.split_whitespace().next().unwrap_or_default();
        let name = program.rsplit('/').next().unwrap_or(program);
        name.strip_prefix('-').unwrap_or(name)
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

    /// The most a listing may print before it is called unreadable: about
    /// 120 times this machine's, measured 10-28 at 140,746 bytes for 793
    /// processes — room for a machine with ten times the processes and
    /// command lines ten times as long, never a host's whole memory.
    const LISTING_MAX_BYTES: u64 = 16 * 1024 * 1024;
    /// How often a running listing is asked whether it has finished: a
    /// quarter of the 20 ms the listing took here (five runs, 10-28), so the
    /// wait past its end is at most 5 ms of the 800 ms wall.
    const LISTING_POLL: Duration = Duration::from_millis(5);

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

impl ProtectedRoot {
    /// The window itself.
    #[must_use]
    pub fn window(pid: u32) -> Self {
        Self {
            pid,
            label: WINDOW_LABEL.to_string(),
        }
    }

    /// What a process inside a ZeroCode pane may not signal, read from `env`
    /// (its value of [`WINDOW_PID_ENV`]): the window, and every process the
    /// window started whose tree does not hold `caller` — the other panes,
    /// the window's own helpers. Empty outside a window. The window's whole
    /// tree is kept either way; the processes it started are named so a
    /// reason can say which one was hit.
    #[must_use]
    pub fn from_window_env(
        env: impl Fn(&str) -> Option<String>,
        table: &ProcessTable,
        caller: u32,
    ) -> Vec<Self> {
        let Some(window) = env(WINDOW_PID_ENV)
            .and_then(|value| value.trim().parse::<u32>().ok())
            .filter(|pid| *pid != 0)
        else {
            return Vec::new();
        };
        let mut kept = vec![Self::window(window)];
        if let Ok(rows) = &table.rows {
            let callers_line = lineage(&by_pid(rows), caller);
            kept.extend(
                rows.iter()
                    .filter(|row| row.ppid == window && !callers_line.contains(&row.pid))
                    .map(|row| Self {
                        pid: row.pid,
                        label: format!(
                            "{} (pid {}), which {WINDOW_LABEL} started",
                            quoted(row.name()),
                            row.pid
                        ),
                    }),
            );
        }
        kept
    }
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
    let mut calls = Vec::new();
    if might_signal(command) {
        calls_in(&script::read(command), &mut calls, 0);
    }
    calls
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
    let mut ask: Option<String> = None;
    let mut by_pid: Option<HashMap<u32, &ProcessRow>> = None;
    for call in calls {
        let written = quoted(&call.written);
        for target in &call.targets {
            let rows = match (target, &table.rows) {
                (KillTarget::OwnShell, _) => continue,
                (KillTarget::Unknown(what), _) => {
                    ask.get_or_insert_with(|| {
                        format!(
                            "`{written}` signals pids it learns only when it runs ({}); a person should say yes first",
                            quoted(what)
                        )
                    });
                    continue;
                }
                (_, Err(why)) => {
                    ask.get_or_insert_with(|| {
                        format!(
                            "`{written}` could not be checked: the process table did not read ({why}); a person should say yes first"
                        )
                    });
                    continue;
                }
                (_, Ok(rows)) => rows,
            };
            let matched = match matching(target, rows) {
                Ok(matched) => matched,
                Err(why) => {
                    ask.get_or_insert_with(|| {
                        format!(
                            "`{written}` could not be checked: {why}; a person should say yes first"
                        )
                    });
                    continue;
                }
            };
            let by_pid = by_pid.get_or_insert_with(|| self::by_pid(rows));
            for row in matched {
                match place(row.pid, by_pid, caller, protected) {
                    Place::Own => {}
                    Place::Kept(label) => {
                        return KillVerdict::Deny {
                            reason: format!(
                                "`{written}` would signal {} (pid {}) in {label}; a pane may signal only what its own pane started",
                                quoted(row.name()),
                                row.pid
                            ),
                        };
                    }
                    Place::Outside => {
                        ask.get_or_insert_with(|| {
                            format!(
                                "`{written}` would signal {} (pid {}), which this pane did not start; a person should say yes first",
                                quoted(row.name()),
                                row.pid
                            )
                        });
                    }
                }
            }
        }
    }
    ask.map_or(KillVerdict::Allow, |reason| KillVerdict::Ask { reason })
}

/* ---- reading the calls ------------------------------------------------------ */

/// Whether `text` holds the letters `kill` in any case — every program this
/// guard reads does, so text without them is not read at all.
#[must_use]
pub fn might_signal(text: &str) -> bool {
    text.as_bytes()
        .windows(4)
        .any(|four| four.eq_ignore_ascii_case(b"kill"))
}

fn calls_in(script: &Script, calls: &mut Vec<KillCall>, depth: usize) {
    if script.overflowed {
        calls.push(KillCall {
            written: "(nested too deep to read)".to_string(),
            targets: vec![KillTarget::Unknown(
                "substitutions nested too deep to read".to_string(),
            )],
        });
    }
    for pipeline in &script.pipelines {
        for (at, command) in pipeline.iter().enumerate() {
            for nested in command.scripts() {
                calls_in(nested, calls, depth);
            }
            call_of(command, &pipeline[..at], calls, depth);
        }
    }
}

/// The call `command` makes, if it is one — through its wrappers, an
/// `xargs` that feeds it the output of `upstream`, a shell's `-c` string or
/// input, an `eval`.
fn call_of(command: &Command, upstream: &[Command], calls: &mut Vec<KillCall>, depth: usize) {
    let mut words = command.words.as_slice();
    let mut fed = false;
    loop {
        let Some(program) = words.first().and_then(Word::literal).map(program_name) else {
            return;
        };
        let args = &words[1..];
        if let Some(rest) = after_wrapper(&program, args) {
            fed |= program == "xargs";
            words = rest;
            continue;
        }
        if SHELL_COMMANDS.contains(&program.as_str()) {
            match shell_string(args) {
                Some(word) => run_text(&word.text, word.expands(), calls, depth),
                None if args
                    .iter()
                    .all(|word| word.literal().is_some_and(|text| text.starts_with('-'))) =>
                {
                    // A shell with no script file runs what it reads.
                    for text in &command.here {
                        run_text(text, false, calls, depth);
                    }
                    if command.here.is_empty()
                        && !fed
                        && let Some((text, expands)) = printed_by(upstream)
                    {
                        run_text(&text, expands, calls, depth);
                    }
                }
                None => {}
            }
            return;
        }
        if program == "eval" {
            let text = args
                .iter()
                .map(|word| word.text.as_str())
                .collect::<Vec<_>>()
                .join(" ");
            run_text(&text, args.iter().any(Word::expands), calls, depth);
            return;
        }
        let targets = match program.as_str() {
            "kill" => kill_targets(args),
            "pkill" => selected(args),
            "killall" => killall_targets(args),
            _ => return,
        };
        let Some(mut targets) = targets else { return };
        if fed {
            targets.extend(pids_from(upstream));
        }
        let mut written = if fed {
            "xargs ".to_string()
        } else {
            String::new()
        };
        written.push_str(&written_words(words));
        calls.push(KillCall { written, targets });
        return;
    }
}

/// A string the shell is handed as a script: read in its turn, or — when
/// part of it is filled in at run time and it names `kill` — a call whose
/// targets are not known.
fn run_text(text: &str, expands: bool, calls: &mut Vec<KillCall>, depth: usize) {
    if !might_signal(text) {
        return;
    }
    if expands || depth >= MAX_STRING_DEPTH {
        calls.push(KillCall {
            written: text.trim().to_string(),
            targets: vec![KillTarget::Unknown(text.trim().to_string())],
        });
        return;
    }
    calls_in(&script::read(text), calls, depth + 1);
}

/// The letters `echo` or `printf` hands a pipe, or the here-document a `cat`
/// without a file hands it (`cat <<'EOF' | bash`), when that is all upstream.
fn printed_by(upstream: &[Command]) -> Option<(String, bool)> {
    let [source] = upstream else { return None };
    let (program, args) = source.words.split_first()?;
    let program = program_name(program.literal()?);
    let args: Vec<&Word> = args
        .iter()
        .skip_while(|word| word.literal().is_some_and(|text| text.starts_with('-')))
        .collect();
    if program == "cat" {
        // A file operand is what `cat` prints instead; `-` is its input.
        return (args.is_empty() && !source.here.is_empty()).then(|| (source.here.concat(), false));
    }
    if program != "echo" && program != "printf" {
        return None;
    }
    let text = args
        .iter()
        .map(|word| word.text.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    Some((text, args.iter().any(|word| word.expands())))
}

/// The program a command word runs: its last path part, ASCII case folded —
/// a case-insensitive disk finds `PKILL` too.
fn program_name(word: &str) -> String {
    word.rsplit('/').next().unwrap_or(word).to_ascii_lowercase()
}

/// The words after a wrapper's own options and arguments, when `program`
/// is a wrapper; `Some(&[])` for a wrapper that only looks a command up.
fn after_wrapper<'w>(program: &str, args: &'w [Word]) -> Option<&'w [Word]> {
    if !WRAPPER_COMMANDS.contains(&program) && !RUNS_THE_NEXT_COMMAND.contains(&program) {
        return None;
    }
    let first = args.first().and_then(Word::literal);
    if program == "command" && matches!(first, Some("-v" | "-V")) {
        return Some(&[]);
    }
    let takes_value = wrapper_value_flags(program);
    let mut at = 0;
    while let Some(text) = args.get(at).and_then(Word::literal) {
        if text == "--" {
            at += 1;
            break;
        }
        if text.len() < 2 || !text.starts_with('-') {
            break;
        }
        at += 1;
        if takes_value.contains(&text) {
            at += 1;
        }
    }
    match program {
        "timeout" => {
            if args
                .get(at)
                .and_then(Word::literal)
                .is_some_and(|text| text.starts_with(|c: char| c.is_ascii_digit()))
            {
                at += 1;
            }
        }
        "env" => {
            while args.get(at).is_some_and(script::is_assignment) {
                at += 1;
            }
        }
        _ => {}
    }
    Some(args.get(at..).unwrap_or_default())
}

/// The options of a wrapper that take the next word as their value.
fn wrapper_value_flags(wrapper: &str) -> &'static [&'static str] {
    match wrapper {
        "sudo" => &[
            "-u", "-g", "-h", "-p", "-C", "-U", "-r", "-t", "-D", "-R", "-T",
        ],
        "doas" => &["-u", "-C"],
        "xargs" => &[
            "-I", "-J", "-L", "-n", "-P", "-R", "-S", "-s", "-E", "-d", "-a",
        ],
        "nice" | "ionice" => &["-n", "-c", "-p"],
        "timeout" => &["-s", "-k", "--signal", "--kill-after"],
        "stdbuf" => &["-i", "-o", "-e"],
        "env" => &["-u", "-C", "-P"],
        "exec" => &["-a"],
        _ => &[],
    }
}

/// The script word of `sh -c '…'`: the first operand after options, when
/// one of them carries `c`.
fn shell_string(args: &[Word]) -> Option<&Word> {
    let mut command_flag = false;
    for word in args {
        match word.literal() {
            Some(text) if text.starts_with('-') && !text.starts_with("--") && text.len() > 1 => {
                command_flag |= text.contains('c');
            }
            Some(text) if text.starts_with("--") => {}
            _ => return command_flag.then_some(word),
        }
    }
    None
}

/// The words of a call, for its reason.
fn written_words(words: &[Word]) -> String {
    words.iter().map(shown).collect::<Vec<_>>().join(" ")
}

/// A word as a reason shows it: its letters, then what it fills in.
fn shown(word: &Word) -> String {
    let mut text = word.text.clone();
    for param in &word.params {
        text.push('$');
        text.push_str(param);
    }
    for _ in &word.subs {
        text.push_str("$(…)");
    }
    if word.arithmetic {
        text.push_str("$((…))");
    }
    text
}

/// `-9`, `-KILL`, `-SIGTERM`: a signal named as an option. Single capital
/// letters are options of their own (`-I`, `-F`).
fn is_signal_option(body: &str) -> bool {
    (!body.is_empty() && body.bytes().all(|b| b.is_ascii_digit()))
        || (body.len() > 1
            && body
                .bytes()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit()))
}

/// Signal 0 only asks whether a process is there.
fn is_probe(signal: &str) -> bool {
    signal == "0"
}

/// `kill`'s targets; `None` when it sends nothing (`kill -l`, `kill -0`).
fn kill_targets(args: &[Word]) -> Option<Vec<KillTarget>> {
    let mut targets = Vec::new();
    let mut options = true;
    let mut signal_given = false;
    let mut at = 0;
    while let Some(word) = args.get(at) {
        at += 1;
        if options && let Some(text) = word.literal() {
            if text == "--" {
                options = false;
                continue;
            }
            if matches!(text, "-l" | "-L") {
                return None;
            }
            if matches!(text, "-s" | "-n") {
                let value = args.get(at).and_then(Word::literal);
                at += 1;
                if value.is_some_and(is_probe) {
                    return None;
                }
                signal_given = true;
                continue;
            }
            if !signal_given && text.len() > 1 && text.starts_with('-') {
                if is_probe(&text[1..]) {
                    return None;
                }
                signal_given = true;
                continue;
            }
        }
        options = false;
        targets.extend(kill_operand(word));
    }
    Some(targets)
}

/// One operand of `kill`: what it signals.
fn kill_operand(word: &Word) -> Vec<KillTarget> {
    if let Some(text) = word.literal() {
        if text.starts_with('%') {
            return vec![KillTarget::OwnShell];
        }
        // A word that is not a number is refused by `kill` itself.
        let Ok(number) = text.parse::<i64>() else {
            return Vec::new();
        };
        let target = match number {
            0 => Some(KillTarget::OwnShell),
            -1 => Some(KillTarget::Everything),
            pid if pid > 0 => u32::try_from(pid).ok().map(KillTarget::Pid),
            group => u32::try_from(-group).ok().map(KillTarget::Group),
        };
        return target.into_iter().collect();
    }
    let bare = word.text.is_empty() && !word.arithmetic;
    if bare
        && word.subs.is_empty()
        && matches!(word.params.as_slice(), [one] if one == "!" || one == "$")
    {
        return vec![KillTarget::OwnShell];
    }
    if bare
        && word.params.is_empty()
        && let [sub] = word.subs.as_slice()
        && let [pipeline] = sub.pipelines.as_slice()
    {
        // `kill $(pgrep -f …)`: the pids of what the pipeline selects.
        return pids_from(pipeline);
    }
    vec![KillTarget::Unknown(shown(word))]
}

/// What the pids a pipeline prints are: the processes `pgrep` selects, the
/// lines `ps | grep PATTERN` keeps, or unknown.
fn pids_from(pipeline: &[Command]) -> Vec<KillTarget> {
    let source = pipeline.first().and_then(|command| {
        let mut words = command.words.as_slice();
        loop {
            let program = program_name(words.first()?.literal()?);
            match after_wrapper(&program, &words[1..]) {
                Some(rest) if program != "xargs" => words = rest,
                _ => return Some((program, &words[1..])),
            }
        }
    });
    match source {
        Some((program, args)) if program == "pgrep" => selected(args).unwrap_or_default(),
        Some((program, _)) if program == "ps" => pipeline[1..]
            .iter()
            .find_map(grep_pattern)
            .map(KillTarget::Matching)
            .map_or_else(
                || vec![KillTarget::Unknown("the output of ps".to_string())],
                |target| vec![target],
            ),
        _ => vec![KillTarget::Unknown(
            pipeline
                .first()
                .map(|command| written_words(&command.words))
                .unwrap_or_else(|| "its input".to_string()),
        )],
    }
}

/// The lines a `grep` keeps, as a pattern on whole command lines; `None`
/// for any other command and for `grep -v`.
fn grep_pattern(command: &Command) -> Option<ProcessPattern> {
    let (program, args) = command.words.split_first()?;
    let program = program_name(program.literal()?);
    if !matches!(program.as_str(), "grep" | "egrep" | "fgrep") {
        return None;
    }
    let mut ignore_case = false;
    let mut regex = program != "fgrep";
    let mut pattern = None;
    let mut at = 0;
    while let Some(word) = args.get(at) {
        at += 1;
        let text = word.literal()?;
        if text == "-e" {
            pattern = args.get(at).and_then(Word::literal);
            break;
        }
        if text.len() > 1 && text.starts_with('-') && !text.starts_with("--") {
            if text.contains('v') {
                return None;
            }
            ignore_case |= text.contains('i');
            regex &= !text.contains('F');
            continue;
        }
        pattern = Some(text);
        break;
    }
    Some(ProcessPattern {
        text: pattern?.to_string(),
        full: true,
        exact: false,
        ignore_case,
        regex,
    })
}

/// `pkill`'s and `pgrep`'s selection; `None` when `pkill` sends signal 0.
fn selected(args: &[Word]) -> Option<Vec<KillTarget>> {
    let (mut full, mut exact, mut ignore_case, mut invert) = (false, false, false, false);
    let mut filtered = false;
    let mut parent: Option<KillTarget> = None;
    let mut pidfile = false;
    let mut patterns: Vec<&Word> = Vec::new();
    let mut options = true;
    let mut at = 0;
    while let Some(word) = args.get(at) {
        at += 1;
        let option = word
            .literal()
            .filter(|text| options && text.len() > 1 && text.starts_with('-'));
        let Some(text) = option else {
            patterns.push(word);
            continue;
        };
        if text == "--" {
            options = false;
            continue;
        }
        if let Some(long) = text.strip_prefix("--") {
            let (name, inline) = long
                .split_once('=')
                .map_or((long, false), |(name, _)| (name, true));
            let value = if inline { None } else { args.get(at) };
            match name {
                "full" => full = true,
                "exact" => exact = true,
                "ignore-case" => ignore_case = true,
                "inverse" => invert = true,
                "signal" | "parent" | "pgroup" | "session" | "euid" | "uid" | "group"
                | "terminal" | "pidfile" | "ns" | "nslist" | "cgroup" | "env" => {
                    if !inline {
                        at += 1;
                    }
                    match name {
                        "signal" if value.and_then(Word::literal).is_some_and(is_probe) => {
                            return None;
                        }
                        "signal" => {}
                        "parent" => parent = value.map(children_of),
                        "pidfile" => pidfile = true,
                        _ => filtered = true,
                    }
                }
                _ => {}
            }
            continue;
        }
        let body = &text[1..];
        if at == 1 && is_signal_option(body) {
            if is_probe(body) {
                return None;
            }
            continue;
        }
        for (index, flag) in body.char_indices() {
            match flag {
                'f' => full = true,
                'x' => exact = true,
                'i' => ignore_case = true,
                'v' => invert = true,
                'F' | 'G' | 'M' | 'N' | 'P' | 'U' | 'c' | 'd' | 'g' | 's' | 't' | 'u' => {
                    let glued = &body[index + flag.len_utf8()..];
                    let value = if glued.is_empty() {
                        at += 1;
                        args.get(at - 1).map(children_of)
                    } else {
                        Some(children_of_text(glued))
                    };
                    match flag {
                        'P' => parent = value,
                        'F' => pidfile = true,
                        'M' | 'N' | 'd' => {}
                        _ => filtered = true,
                    }
                    break;
                }
                _ => {}
            }
        }
    }
    if invert {
        return Some(vec![KillTarget::Everything]);
    }
    let mut targets: Vec<KillTarget> = patterns
        .into_iter()
        .map(|word| match word.literal() {
            Some(text) => KillTarget::Matching(ProcessPattern {
                text: text.to_string(),
                full,
                exact,
                ignore_case,
                regex: true,
            }),
            None => KillTarget::Unknown(shown(word)),
        })
        .collect();
    if pidfile {
        targets.push(KillTarget::Unknown("a pid file".to_string()));
    }
    if targets.is_empty() {
        if let Some(parent) = parent {
            targets.push(parent);
        } else if filtered {
            targets.push(KillTarget::Everything);
        }
    }
    Some(targets)
}

/// `-P`'s value: the children of that process, or unknown.
fn children_of(word: &Word) -> KillTarget {
    word.literal()
        .map_or_else(|| KillTarget::Unknown(shown(word)), children_of_text)
}

fn children_of_text(text: &str) -> KillTarget {
    text.parse().map_or_else(
        |_| KillTarget::Unknown(text.to_string()),
        KillTarget::ChildrenOf,
    )
}

/// `killall`'s targets; `None` when it sends nothing (`-l`, `-d`, the macOS
/// `-s` that only shows, signal 0).
fn killall_targets(args: &[Word]) -> Option<Vec<KillTarget>> {
    let (mut regex, mut ignore_case, mut filtered) = (false, false, false);
    let mut names: Vec<&Word> = Vec::new();
    let mut options = true;
    let mut at = 0;
    while let Some(word) = args.get(at) {
        at += 1;
        let option = word
            .literal()
            .filter(|text| options && text.len() > 1 && text.starts_with('-'));
        let Some(text) = option else {
            names.push(word);
            continue;
        };
        if text == "--" {
            options = false;
            continue;
        }
        let body = text.trim_start_matches('-');
        if is_signal_option(body) {
            if is_probe(body) {
                return None;
            }
            continue;
        }
        match body {
            "l" | "d" | "help" => return None,
            "s" => {
                // Linux takes a signal here; macOS only shows what it would do.
                let value = args.get(at).and_then(Word::literal);
                if !value.is_some_and(is_signal_option) {
                    return None;
                }
                at += 1;
                if value.is_some_and(is_probe) {
                    return None;
                }
            }
            "m" | "r" | "regexp" => regex = true,
            "I" | "ignore-case" => ignore_case = true,
            "u" | "t" | "g" | "user" => {
                at += 1;
                filtered = true;
            }
            "c" => {
                if let Some(value) = args.get(at) {
                    names.push(value);
                }
                at += 1;
            }
            "y" | "o" | "n" => at += 1,
            _ => {}
        }
    }
    let mut targets: Vec<KillTarget> = names
        .into_iter()
        .map(|word| match word.literal() {
            Some(text) => KillTarget::Matching(ProcessPattern {
                text: text.to_string(),
                full: false,
                exact: !regex,
                ignore_case,
                regex,
            }),
            None => KillTarget::Unknown(shown(word)),
        })
        .collect();
    if targets.is_empty() && filtered {
        targets.push(KillTarget::Everything);
    }
    Some(targets)
}

/* ---- judging them ----------------------------------------------------------- */

/// Where a process hangs, seen from the calling pane.
enum Place<'a> {
    /// In the calling pane's own tree.
    Own,
    /// In a kept tree, and not the caller's.
    Kept(&'a str),
    /// In neither.
    Outside,
}

/// Walk up from `pid`: the first of the caller and the kept roots met
/// decides, so a pane inside the window's tree is its own and not the
/// window's.
fn place<'a>(
    pid: u32,
    by_pid: &HashMap<u32, &ProcessRow>,
    caller: Option<u32>,
    protected: &'a [ProtectedRoot],
) -> Place<'a> {
    for at in lineage(by_pid, pid) {
        if caller == Some(at) {
            return Place::Own;
        }
        if let Some(root) = protected.iter().find(|root| root.pid == at) {
            return Place::Kept(&root.label);
        }
    }
    Place::Outside
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

/// The rows `target` selects; an error for a pattern that does not read.
fn matching<'r>(
    target: &KillTarget,
    rows: &'r [ProcessRow],
) -> Result<Vec<&'r ProcessRow>, String> {
    let select = |keep: &dyn Fn(&ProcessRow) -> bool| -> Vec<&'r ProcessRow> {
        rows.iter().filter(|row| keep(row)).collect()
    };
    Ok(match target {
        KillTarget::Pid(pid) => select(&|row| row.pid == *pid),
        KillTarget::Group(group) => select(&|row| row.pgid == *group),
        KillTarget::ChildrenOf(parent) => select(&|row| row.ppid == *parent),
        KillTarget::Everything => rows.iter().collect(),
        KillTarget::Matching(pattern) => {
            let matcher = Matcher::of(pattern)?;
            select(&|row| {
                matcher.matches(if pattern.full {
                    row.command.as_str()
                } else {
                    row.name()
                })
            })
        }
        KillTarget::OwnShell | KillTarget::Unknown(_) => Vec::new(),
    })
}

/// A pattern made ready to match.
enum Matcher<'p> {
    Regex(regex::Regex),
    Name(&'p ProcessPattern),
}

impl<'p> Matcher<'p> {
    fn of(pattern: &'p ProcessPattern) -> Result<Self, String> {
        if !pattern.regex {
            return Ok(Self::Name(pattern));
        }
        let expression = if pattern.exact {
            format!("^(?:{})$", pattern.text)
        } else {
            pattern.text.clone()
        };
        regex::RegexBuilder::new(&expression)
            .case_insensitive(pattern.ignore_case)
            .size_limit(1 << 20)
            .build()
            .map(Self::Regex)
            .map_err(|_| format!("the pattern {} does not read", quoted(&pattern.text)))
    }

    fn matches(&self, text: &str) -> bool {
        match self {
            Self::Regex(regex) => regex.is_match(text),
            Self::Name(pattern) if pattern.ignore_case => text.eq_ignore_ascii_case(&pattern.text),
            Self::Name(pattern) => text == pattern.text,
        }
    }
}

/// `text` cut to [`QUOTED_CHARS`] for a one-line reason.
fn quoted(text: &str) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() <= QUOTED_CHARS {
        return one_line;
    }
    let mut cut: String = one_line.chars().take(QUOTED_CHARS).collect();
    cut.push('…');
    cut
}

#[cfg(test)]
mod tests;
