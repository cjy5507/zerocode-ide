//! Who leaving this window would cut (t-6428) — the one census every road
//! out of the window asks before it goes, and the goodbye writes down as it
//! goes.
//!
//! Traycer's rule, carried over: a restart asks the running work first, and
//! "I do not know" counts as busy (its `host.restart` answers
//! `busy{workingAgents, runningTerminals}`, and a `null` there is a reason
//! nobody could read, never "nothing"). The census reads two facts for every
//! live worker seated in this window — the workers the 「새 빌드 준비됨」
//! notice has counted since t-3058:
//!
//! - its **turn**, as the window last measured it off the pane's own hooks
//!   ([`super::pane_turns`], the mail pointer's three-valued map): under way,
//!   at rest, or never heard;
//! - the **commands** its agent runs under the pane
//!   ([`ProcessSample::agent_commands`]): a turn at rest can still be
//!   waiting on a background gate or a browser suite, and those are what a
//!   restart used to cut without a word.
//!
//! Busy is a turn under way, a command under a pane at rest, or a fact the
//! window could not read. Commands under a turn that is under way belong to
//! that turn: the census counts the turn once, and names its commands only in
//! the goodbye's own lines.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use super::PaneTurn;
use crate::resource_usage::ProcessSample;

/// How much of a command line the census says: enough to know the command,
/// never a paragraph, and never a value a credential could be.
const COMMAND_SUMMARY_CHARS: usize = 80;

/// One live worker seated in this window — a row the ledger calls live and
/// a pane this window's team table maps to a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Seated {
    pub(crate) worker: String,
    pub(crate) agent: String,
    pub(crate) term: u32,
}

/// What the window last measured of one worker's turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Turn {
    /// Under way, or a question of its own parked in it.
    Running,
    /// Ended, however it ended: the agent is not in a turn.
    Rest,
    /// Nothing heard from the pane since this window began — an agent whose
    /// hooks never report, or a pane that has not reported yet.
    Unheard,
}

impl Turn {
    fn of(heard: Option<PaneTurn>) -> Self {
        match heard {
            Some(PaneTurn::Running) => Self::Running,
            Some(PaneTurn::Ended { .. }) => Self::Rest,
            None => Self::Unheard,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Self::Running => "running",
            Self::Rest => "rest",
            Self::Unheard => "unheard",
        }
    }
}

/// One live worker, as the census found it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WorkerCut {
    pub(crate) worker: String,
    pub(crate) agent: String,
    pub(crate) term: u32,
    pub(crate) turn: Turn,
    /// What its agent runs under the pane, each command summarized
    /// ([`command_summary`]) — or `None` when the window could not read it:
    /// no root process for the terminal, or no table to read it in.
    pub(crate) commands: Option<Vec<String>>,
}

/// Every live worker seated in this window, read at one moment.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RestartCensus {
    pub(crate) workers: Vec<WorkerCut>,
    /// What the reading cost, for the goodbye's line.
    pub(crate) took_ms: u64,
}

/// The census as every asker speaks it: whether to ask at all, and the
/// three numbers the question says.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Busy {
    pub(crate) busy: bool,
    pub(crate) workers: usize,
    /// Workers whose turn is under way.
    pub(crate) turning: usize,
    /// Commands still running under panes whose turn is at rest.
    pub(crate) background: usize,
    /// Workers the window could not read: never heard, or no table.
    pub(crate) unknown: usize,
    /// Commands running under every worker's pane, whatever its turn —
    /// what 「끝나면」 waits out.
    pub(crate) running: usize,
    /// The first gap 「끝나면」 leaves at: nothing runs under any worker's
    /// pane and nothing is unread. A turn under way with nothing running
    /// under it is a gap — the cut is its reply in flight, which the
    /// restart nudge picks up; a command cut is work done again.
    pub(crate) gap: bool,
}

impl Default for Busy {
    /// Nobody seated: nothing to ask about, and nothing to wait out.
    fn default() -> Self {
        Self {
            busy: false,
            workers: 0,
            turning: 0,
            background: 0,
            unknown: 0,
            running: 0,
            gap: true,
        }
    }
}

impl RestartCensus {
    pub(crate) fn busy(&self) -> Busy {
        let mut said = Busy {
            workers: self.workers.len(),
            ..Busy::default()
        };
        for one in &self.workers {
            match (one.turn, &one.commands) {
                (Turn::Running, _) => said.turning += 1,
                (Turn::Unheard, _) | (Turn::Rest, None) => said.unknown += 1,
                (Turn::Rest, Some(commands)) => said.background += commands.len(),
            }
            said.running += one.commands.as_ref().map_or(0, Vec::len);
        }
        said.busy = said.turning + said.unknown + said.background > 0;
        said.gap = self.workers.iter().all(|one| {
            one.turn != Turn::Unheard && one.commands.as_ref().is_some_and(Vec::is_empty)
        });
        said
    }
}

/// Read the census. `root_of` answers the process at the root of a terminal
/// (the window's pty table); `table` reads the host's process table, once,
/// and only when some worker is seated here at all.
pub(crate) fn take(
    root_of: &dyn Fn(u32) -> Option<u32>,
    table: &dyn Fn() -> Result<ProcessSample, String>,
) -> RestartCensus {
    let started = Instant::now();
    let seated = super::seated_live_workers();
    if seated.is_empty() {
        return RestartCensus::default();
    }
    let turns: HashMap<u32, PaneTurn> = {
        let held = super::pane_turns()
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        seated
            .iter()
            .filter_map(|one| held.get(&one.term).map(|turn| (one.term, *turn)))
            .collect()
    };
    let sample = table().ok();
    let workers = seated
        .into_iter()
        .map(|one| {
            let commands = root_of(one.term)
                .zip(sample.as_ref())
                .and_then(|(root, sample)| sample.agent_commands(root, &agent_body(&one.agent)))
                .map(|found| {
                    found
                        .iter()
                        .map(|command| command_summary(&command.command))
                        .collect()
                });
            WorkerCut {
                turn: Turn::of(turns.get(&one.term).copied()),
                commands,
                worker: one.worker,
                agent: one.agent,
                term: one.term,
            }
        })
        .collect();
    RestartCensus {
        workers,
        took_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
    }
}

/// The names an agent's own processes run under — the catalog row's process,
/// its command and the command's other names — so a launcher's child that is
/// the agent itself is walked through, and a helper is not.
fn agent_body(agent: &str) -> Vec<&'static str> {
    zerocode_core::agent_spec(agent).map_or_else(Vec::new, |spec| {
        let mut names = vec![spec.expected_process, spec.detect];
        names.extend_from_slice(spec.detect_aliases);
        names
    })
}

/// Shells a tool's command rides in: the command is the script after `-c`.
const SHELLS: [&str; 6] = ["sh", "bash", "zsh", "dash", "fish", "ksh"];
/// Claude Code's tool shell runs the command it was given through
/// `eval '…'`, after setup of its own.
const EVAL_OPEN: &str = "eval '";
/// Words that set a command up rather than being it: `cd x && cargo test`
/// is `cargo test`.
const PREAMBLE: [&str; 7] = ["cd", "export", "source", ".", "set", "setopt", "pushd"];
/// Where one simple command ends inside a script.
const SEPARATORS: [&str; 4] = ["&&", "||", ";", "|"];

/// A command line said in a few words: the command a tool's shell carries
/// rather than the shell, its program by name rather than by path, no
/// leading assignment or redirection, and every value that could be a
/// credential masked by the one table that knows them
/// (`zerocode_core::credential`).
pub(crate) fn command_summary(command: &str) -> String {
    let script = shell_script(command).unwrap_or(command);
    let script = script.split_once(EVAL_OPEN).map_or(script, |(_, evaled)| {
        evaled.split('\'').next().unwrap_or(evaled)
    });
    let mut said = zerocode_core::credential::mask_values(&first_command(script).join(" "));
    if said.is_empty() {
        said = command
            .split_ascii_whitespace()
            .next()
            .map(file_name)
            .unwrap_or_default()
            .to_string();
    }
    if said.chars().count() > COMMAND_SUMMARY_CHARS {
        said = said.chars().take(COMMAND_SUMMARY_CHARS - 1).collect();
        said.push('…');
    }
    said
}

/// The script a shell was handed with `-c` (or `-lc`, `-ic`…), when the
/// command is a shell running one.
fn shell_script(command: &str) -> Option<&str> {
    let mut words = command.split_ascii_whitespace();
    if !SHELLS.contains(&file_name(words.next()?)) {
        return None;
    }
    for word in words {
        let flags = word.strip_prefix('-').filter(|flags| {
            !flags.is_empty() && flags.chars().all(|flag| flag.is_ascii_alphabetic())
        })?;
        if flags.contains('c') {
            // A word is a slice of the line, so where it ends is where the
            // script begins.
            let after = word.as_ptr() as usize - command.as_ptr() as usize + word.len();
            return Some(command[after..].trim_start());
        }
    }
    None
}

/// The first simple command in a script that is not setting another up,
/// without its leading assignments and from its first redirection on.
fn first_command(script: &str) -> Vec<&str> {
    let mut segments: Vec<Vec<&str>> = vec![Vec::new()];
    for word in script.split_ascii_whitespace() {
        let (word, ends) = word
            .strip_suffix(';')
            .map_or((word, false), |bare| (bare, true));
        if SEPARATORS.contains(&word) {
            segments.push(Vec::new());
            continue;
        }
        if !word.is_empty()
            && let Some(segment) = segments.last_mut()
        {
            segment.push(word);
        }
        if ends {
            segments.push(Vec::new());
        }
    }
    segments
        .into_iter()
        .map(|segment| {
            segment
                .into_iter()
                .skip_while(|word| is_assignment(word))
                .take_while(|word| !is_redirection(word))
                .collect::<Vec<_>>()
        })
        .find(|segment| {
            segment
                .first()
                .is_some_and(|program| !PREAMBLE.contains(program))
        })
        .map(|mut segment| {
            segment[0] = file_name(segment[0]);
            segment
        })
        .unwrap_or_default()
}

/// `NAME=value` before a command.
fn is_assignment(word: &str) -> bool {
    word.split_once('=').is_some_and(|(name, _)| {
        name.chars()
            .next()
            .is_some_and(|first| first.is_ascii_alphabetic() || first == '_')
            && name
                .chars()
                .all(|held| held.is_ascii_alphanumeric() || held == '_')
    })
}

/// `>file`, `2>&1`, `&>/dev/null`, `<input`.
fn is_redirection(word: &str) -> bool {
    word.trim_start_matches(|held: char| held.is_ascii_digit() || held == '&')
        .starts_with(['<', '>'])
}

/// A program by its file name.
fn file_name(program: &str) -> &str {
    program.rsplit(['/', '\\']).next().unwrap_or(program)
}

/// Where the goodbye leaves each worker's cut commands for its wake
/// (t-6428 ⑤): beside the window's log, in the local data root.
const CUT_FILE: &str = "restart-cut.json";

/// What the goodbye left for the wakes, loaded once per data root per
/// process: the old window writes it on its way out, and the new one reads
/// it at its first wake.
type CutBook = HashMap<PathBuf, HashMap<String, Vec<String>>>;

fn cut_book() -> &'static Mutex<CutBook> {
    static BOOK: OnceLock<Mutex<CutBook>> = OnceLock::new();
    BOOK.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Leave each worker's cut commands for its wake (t-6428 ⑤): the workers
/// that had commands running under their panes as the window went. A
/// goodbye that cut nothing leaves no file — not even an older one.
pub(crate) fn leave_cut(root: &Path, census: &RestartCensus) -> std::io::Result<()> {
    let cut: BTreeMap<&str, &Vec<String>> = census
        .workers
        .iter()
        .filter_map(|one| {
            one.commands
                .as_ref()
                .filter(|commands| !commands.is_empty())
                .map(|commands| (one.worker.as_str(), commands))
        })
        .collect();
    let path = root.join(CUT_FILE);
    if cut.is_empty() {
        return match crate::durable_file::remove_file(&path) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        };
    }
    let bytes = serde_json::to_vec(&cut).map_err(std::io::Error::other)?;
    crate::durable_file::replace_bytes(&path, &bytes).map(|_| ())
}

/// The commands the goodbye cut under this worker's pane, said once
/// (t-6428 ⑤). The first wake after a boot takes the file the goodbye left
/// and removes it; each worker's entry goes to its own wake and no other.
pub(crate) fn take_cut(root: &Path, worker: &str) -> Vec<String> {
    let mut book = cut_book().lock().unwrap_or_else(|held| held.into_inner());
    book.entry(root.to_path_buf())
        .or_insert_with(|| load_cut(root))
        .remove(worker)
        .unwrap_or_default()
}

/// The note as the goodbye left it, taken off the disk: nothing, or a note
/// nobody can read, is nothing cut.
fn load_cut(root: &Path) -> HashMap<String, Vec<String>> {
    let path = root.join(CUT_FILE);
    let loaded = crate::durable_file::read_plain_file(&path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();
    let _ = crate::durable_file::remove_file(&path);
    loaded
}

/// The goodbye's lines (t-6428 ①): one for the road, the census's numbers
/// at the moment the window actually went and what the person chose when
/// asked, then one per worker with its turn and what ran under it — the
/// before-and-after measure of what leaving the window cut.
pub(crate) fn goodbye_lines(road: &str, choice: &str, census: &RestartCensus) -> Vec<String> {
    let busy = census.busy();
    let mut lines = vec![format!(
        "exit by {road} · workers {} · mid-turn {} · background {} · running {} · unknown {} · \
         busy {} · choice {choice} · census {} ms",
        busy.workers,
        busy.turning,
        busy.background,
        busy.running,
        busy.unknown,
        if busy.busy { "yes" } else { "no" },
        census.took_ms
    )];
    for one in &census.workers {
        let under = match &one.commands {
            Some(commands) if commands.is_empty() => "0 command(s)".to_string(),
            Some(commands) => format!("{} command(s): {}", commands.len(), commands.join("; ")),
            None => "commands unread".to_string(),
        };
        lines.push(format!(
            "exit: worker {} on terminal {} ({}) · turn {} · {under}",
            one.worker,
            one.term,
            one.agent,
            one.turn.word()
        ));
    }
    lines
}

#[cfg(test)]
mod tests {
    use super::*;

    fn worker(turn: Turn, commands: Option<&[&str]>) -> WorkerCut {
        WorkerCut {
            worker: "w-1".to_string(),
            agent: "claude".to_string(),
            term: 41,
            turn,
            commands: commands.map(|held| held.iter().map(ToString::to_string).collect()),
        }
    }

    #[test]
    fn a_turn_under_way_a_command_at_rest_and_an_unread_fact_are_each_busy() {
        let census = |workers: Vec<WorkerCut>| {
            RestartCensus {
                workers,
                took_ms: 0,
            }
            .busy()
        };
        // Nothing seated: nothing to ask.
        assert_eq!(census(Vec::new()), Busy::default());
        // At rest with nothing under it: a restart cuts nothing.
        let idle = census(vec![worker(Turn::Rest, Some(&[]))]);
        assert!(!idle.busy, "{idle:?}");
        assert_eq!(idle.workers, 1);
        // Under way: the turn is counted once, its own commands are its.
        let turning = census(vec![worker(Turn::Running, Some(&["cargo test"]))]);
        assert!(turning.busy);
        assert_eq!((turning.turning, turning.background), (1, 0));
        // At rest with a gate running under it: background.
        let gate = census(vec![worker(Turn::Rest, Some(&["cargo test", "just gate"]))]);
        assert!(gate.busy);
        assert_eq!((gate.turning, gate.background), (0, 2));
        // Never heard, or a table nobody could read: unknown is busy.
        let unheard = census(vec![worker(Turn::Unheard, Some(&[]))]);
        let unread = census(vec![worker(Turn::Rest, None)]);
        assert!(unheard.busy && unread.busy);
        assert_eq!((unheard.unknown, unread.unknown), (1, 1));
    }

    #[test]
    fn the_gap_is_nothing_running_under_any_worker_and_nothing_unread() {
        let census = |workers: Vec<WorkerCut>| {
            RestartCensus {
                workers,
                took_ms: 0,
            }
            .busy()
        };
        // A turn under way with nothing running under it: busy, and a gap.
        let thinking = census(vec![worker(Turn::Running, Some(&[]))]);
        assert!(thinking.busy && thinking.gap, "{thinking:?}");
        // A command under a turn, or under a turn at rest: no gap.
        let gate = census(vec![
            worker(Turn::Running, Some(&["cargo test"])),
            worker(Turn::Rest, Some(&["just gate"])),
        ]);
        assert!(!gate.gap);
        assert_eq!((gate.running, gate.background), (2, 1));
        // Never heard, or nothing read: no gap — unknown is busy.
        assert!(!census(vec![worker(Turn::Unheard, Some(&[]))]).gap);
        assert!(!census(vec![worker(Turn::Running, None)]).gap);
        // Nobody seated, or everyone at rest with nothing under them.
        assert!(census(Vec::new()).gap);
        let idle = census(vec![worker(Turn::Rest, Some(&[]))]);
        assert!(idle.gap && !idle.busy);
    }

    #[test]
    fn the_goodbye_leaves_each_workers_cut_commands_for_its_own_wake_once() {
        let root = tempfile::tempdir().expect("a data root");
        let file = root.path().join(CUT_FILE);
        let named = |id: &str, commands: Option<&[&str]>| WorkerCut {
            worker: id.to_string(),
            ..worker(Turn::Rest, commands)
        };
        let census = RestartCensus {
            workers: vec![
                named("w-1", Some(&["cargo test -p zerocode-shell", "just gate"])),
                named("w-2", Some(&[])),
                named("w-3", None),
            ],
            took_ms: 0,
        };
        leave_cut(root.path(), &census).expect("the goodbye leaves its note");
        assert!(file.exists(), "nothing was left for the wakes");
        assert_eq!(
            take_cut(root.path(), "w-1"),
            vec!["cargo test -p zerocode-shell", "just gate"]
        );
        assert!(!file.exists(), "the first wake takes the note");
        assert!(take_cut(root.path(), "w-1").is_empty(), "said once");
        assert!(take_cut(root.path(), "w-2").is_empty());
        assert!(take_cut(root.path(), "w-3").is_empty());
        // A goodbye that cut nothing leaves nothing, not even an older note.
        let later = tempfile::tempdir().expect("another data root");
        leave_cut(later.path(), &census).expect("a note");
        leave_cut(
            later.path(),
            &RestartCensus {
                workers: vec![named("w-4", Some(&[]))],
                took_ms: 0,
            },
        )
        .expect("no note");
        assert!(!later.path().join(CUT_FILE).exists());
    }

    #[test]
    fn a_command_is_said_in_a_few_words_without_its_shell_or_a_credential() {
        // Claude Code's tool shell: the command is the one it evals.
        assert_eq!(
            command_summary(
                "/bin/zsh -c source '/Users/dev/.claude/shell-snapshots/snapshot-zsh-1.sh' \
                 2>/dev/null || true && setopt NO_EXTENDED_GLOB 2>/dev/null || true && \
                 eval 'CARGO_INCREMENTAL=0 cargo test -p zerocode-shell 2>&1 | tail -5' \
                 < /dev/null && pwd -P >| /tmp/claude-1-cwd"
            ),
            "cargo test -p zerocode-shell"
        );
        // Codex's and zo's: the script after `-lc`, past a `cd … &&`.
        assert_eq!(
            command_summary("/bin/zsh -lc cd /Users/dev/checkout && just shell-test"),
            "just shell-test"
        );
        assert_eq!(
            command_summary("sh -lc node ui/tests/window.mjs --suite update"),
            "node ui/tests/window.mjs --suite update"
        );
        // A shell that exec'd its command: the program by name.
        assert_eq!(
            command_summary("/Users/dev/.cargo/bin/cargo clippy --all-targets -- -D warnings"),
            "cargo clippy --all-targets -- -D warnings"
        );
        // A credential never rides: the table masks the value it names.
        let said = command_summary("curl -H Authorization: --token ghp_abcdefghijklmnop https://x");
        assert!(!said.contains("ghp_abcdefghijklmnop"), "{said}");
        // A paragraph is cut to a line.
        let long = format!("node {}", "x".repeat(400));
        assert!(command_summary(&long).chars().count() <= COMMAND_SUMMARY_CHARS);
    }

    #[test]
    fn the_goodbye_says_its_road_its_numbers_and_each_worker_once() {
        let census = RestartCensus {
            workers: vec![
                WorkerCut {
                    worker: "w-7".to_string(),
                    agent: "claude".to_string(),
                    term: 41,
                    turn: Turn::Rest,
                    commands: Some(vec!["cargo test -p zerocode-shell".to_string()]),
                },
                WorkerCut {
                    worker: "w-8".to_string(),
                    agent: "codex".to_string(),
                    term: 42,
                    turn: Turn::Running,
                    commands: None,
                },
            ],
            took_ms: 38,
        };
        let lines = goodbye_lines("close", "gap", &census);
        assert_eq!(lines.len(), 3, "{lines:#?}");
        // The worker mid-turn is counted once, as a turn, whatever could be
        // read under it; its line says what could not.
        assert_eq!(
            lines[0],
            "exit by close · workers 2 · mid-turn 1 · background 1 · running 1 · unknown 0 · \
             busy yes · choice gap · census 38 ms"
        );
        assert_eq!(
            lines[1],
            "exit: worker w-7 on terminal 41 (claude) · turn rest · 1 command(s): \
             cargo test -p zerocode-shell"
        );
        assert_eq!(
            lines[2],
            "exit: worker w-8 on terminal 42 (codex) · turn running · commands unread"
        );
        // Nobody seated: one line, so every exit is counted.
        assert_eq!(
            goodbye_lines("terminate", "unasked", &RestartCensus::default()),
            vec![
                "exit by terminate · workers 0 · mid-turn 0 · background 0 · running 0 · unknown 0 · \
                 busy no · choice unasked · census 0 ms"
                    .to_string()
            ]
        );
    }
}
