//! What an agent has to be told before it can actually work.
//!
//! This is the difference between "the agent starts" and "the agent connects".
//! A coding agent run from a terminal defends itself: before it edits a file or
//! runs a command it stops and asks. That is correct at a prompt and useless
//! inside an IDE — the pane shows a question nobody is watching, and the work
//! never happens. So every agent ships a way to say "I have already decided",
//! and each of them spells it differently.
//!
//! Orca's tables, measured verbatim (`YOLO_TUI_AGENT_ARGS`/`YOLO_TUI_AGENT_ENV`
//! /`UNSUPPORTED_TUI_AGENT_ARGS`, I18nProvider-4EBrmTGg.js:23049-23176). They
//! are facts about those tools' own command lines rather than anything of
//! Orca's, which is why they can be carried across as they are.
//!
//! Two things follow that are not obvious:
//!
//!   - **These are the DEFAULTS.** `DEFAULT_TUI_AGENT_ARGS = YOLO_TUI_AGENT_ARGS`
//!     (:23175). Orca hands an agent the bypass flag unless the person has said
//!     otherwise, because the alternative is a product whose agents hang on
//!     first use. We keep that default AND make it visible: see [`PermissionMode`],
//!     which exists so a window can say out loud what a launch will be allowed
//!     to do rather than leaving it in a table.
//!   - **Two agents REJECT the flag.** `opencode` and `kilo` fail to start when
//!     handed `--dangerously-skip-permissions`, so an override carrying it has
//!     it stripped rather than being passed through to a launch that dies
//!     (`sanitizeTuiAgentLaunchArgs`).

use serde::{Deserialize, Serialize};

/// The flag each agent understands as "do not stop to ask me".
///
/// Verbatim from the measurement, in Orca's own order. An agent absent from
/// this table has no such flag — `aug`, `codebuff`, `droid`, `omp`, `pi`,
/// `mimo-code`, `trae`, `opencode`, `kilo`, `goose` — and for one of them the
/// answer is an environment variable instead (see [`AGENT_LAUNCH_ENV`]).
pub const AGENT_LAUNCH_ARGS: &[(&str, &str)] = &[
    // This window's own harness comes first, as it does in `AGENT_SPECS`.
    // Not from Orca's measurement: measured on `zo` 0.1.0 (zo-ide) in a pty.
    // Without this line a summoned zo pane inherits the project's configured
    // mode (`.zo/settings.local.json` can say read-only) or, in a fresh
    // worktree, stops at the first-visit trust dialog that then swallows the
    // pasted briefing — either way the worker cannot run `zerocode-orc` and
    // the ledger only ever hears `went_quiet: never_spoke`. An explicit
    // `--permission-mode` skips both (zo-ide `main.rs`, `permission_mode_explicit`).
    ("zo", "--permission-mode danger-full-access"),
    ("claude", "--dangerously-skip-permissions"),
    ("openclaude", "--dangerously-skip-permissions"),
    ("codex", "--dangerously-bypass-approvals-and-sandbox"),
    ("antigravity", "--dangerously-skip-permissions"),
    ("aider", "--yes-always"),
    ("amp", "--dangerously-allow-all"),
    ("kiro", "--trust-all-tools"),
    ("crush", "--yolo"),
    ("autohand", "--unrestricted"),
    ("cline", "--auto-approve true"),
    ("command-code", "--yolo"),
    ("continue", "--allow \"*\""),
    ("cursor", "--yolo"),
    ("kimi", "--yolo"),
    ("mistral-vibe", "--agent auto-approve"),
    ("qwen-code", "--approval-mode yolo"),
    ("rovo", "--yolo"),
    ("hermes", "--yolo"),
    ("copilot", "--yolo"),
    ("grok", "--permission-mode bypassPermissions"),
    ("devin", "--permission-mode bypass"),
    ("ante", "--yolo"),
];

/// The same thing said as an environment variable, for the agent that has no
/// flag for it. One entry, measured (`YOLO_TUI_AGENT_ENV`: goose).
pub const AGENT_LAUNCH_ENV: &[(&str, &[(&str, &str)])] = &[("goose", &[("GOOSE_MODE", "auto")])];

/// Flags an agent refuses. Passing one is not a permission choice, it is a
/// launch that fails — so an override carrying it is trimmed rather than
/// honoured (`UNSUPPORTED_TUI_AGENT_ARGS`).
pub const AGENT_REFUSED_ARGS: &[(&str, &[&str])] = &[
    ("opencode", &["--dangerously-skip-permissions"]),
    ("kilo", &["--dangerously-skip-permissions"]),
];

/// What keeps an agent's CLI STARTING inside this window — not a permission,
/// so it rides every launch, override or not, and never enters the
/// permission judgement or the field the settings pane shows and saves.
///
/// `codex`, measured on codex-cli 0.157.1: the TUI attaches to a shared
/// background app-server over a unix socket at
/// `<CODEX_HOME>/app-server-control/app-server-control.sock`. Under the
/// window's managed runtime home that path is 122 bytes where macOS's
/// `sun_path` holds 104, the connect is refused ("path must be shorter than
/// SUN_LEN") and the tab ended with code 1 about ten seconds after it opened.
/// A config override is the version-safe spelling: a Codex that predates the
/// key warns that it is ignoring an unrecognised setting and starts, where
/// `--no-daemon` is refused by one that predates the flag. With the key off
/// the TUI runs embedded (measured the same as `--no-daemon`); the window's
/// own app-server sidecars are explicit `--remote` roads and never touch the
/// daemon.
pub const AGENT_LAUNCH_COMPAT_ARGS: &[(&str, &str)] =
    &[("codex", "-c features.daemon_auto_start=false")];

/// The compatibility words for one agent, split the way the process wants
/// them; empty for every agent the table does not name.
#[must_use]
pub fn compat_launch_args(agent: &str) -> Vec<String> {
    AGENT_LAUNCH_COMPAT_ARGS
        .iter()
        .find(|(id, _)| *id == agent)
        .map(|(_, args)| args.split_whitespace().map(str::to_string).collect())
        .unwrap_or_default()
}

/// What keeps the commands an interactive agent runs in the terminal it was
/// started in — the pane's key, window, hook port and orchestration seat.
///
/// Every interactive run in a ZeroCode terminal wears these, typed into a
/// shell or launched by the window, because the terminal cannot tell the two
/// apart and both need the same thing (t-97015). Unlike
/// [`AGENT_LAUNCH_COMPAT_ARGS`] they are not part of any launch plan: the
/// mirror in front of the agent on every pane's `PATH` adds them as the run
/// starts ([`with_terminal_args`]), so a `codex` a person types gets them too.
///
/// `codex`, read from codex-cli 0.167.0's source and measured in a sandbox:
/// the TUI hands its tool commands to a shared app-server daemon under
/// `CODEX_HOME` whenever one answers there (`maybe_probe_default_daemon_socket`,
/// asked whatever `features.daemon_auto_start` says), and by default starts
/// one itself when none does. That daemon keeps the environment of the
/// terminal that first started it, so `zerocode-orc` inside a later pane's
/// Codex read a gone pane's key, a gone window and no seat at all ("this
/// shell is not part of an agent team"). `--no-daemon` — "Run without the
/// shared background server, even if it is already running", since 0.156.0 —
/// keeps the app-server inside the TUI, whose commands then inherit the TUI's
/// own environment. The compatibility key does not do that: against a running
/// daemon the TUI still attaches, and then offers to restart the daemon.
pub const AGENT_TERMINAL_ARGS: &[TerminalArgs] = &[TerminalArgs {
    agent: "codex",
    args: "--no-daemon",
    // `--remote` names another server, and codex refuses the two together
    // ("--no-daemon cannot be used with --remote.").
    chosen_by: &["--no-daemon", "--remote"],
    // The daemon is the TUI's, and these two are the TUI on a saved
    // conversation: 0.167.0's own hint is "rerun the same command with
    // --no-daemon (including resume or fork and its arguments)". Every other
    // subcommand codex lists starts exactly as typed — nobody has run one
    // with the flag, and `queue` and `remote-control <sub>` refuse it.
    interactive_commands: &["resume", "fork"],
}];

/// One agent's entry in [`AGENT_TERMINAL_ARGS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalArgs {
    /// The catalogue id.
    pub agent: &'static str,
    /// The words, put IN FRONT of the run's own: a root flag has to come
    /// before a subcommand to be read as one.
    pub args: &'static str,
    /// Flags that mean the run has already chosen its server, spelled alone
    /// or as `flag=value`. Such a run is left exactly as it was started.
    pub chosen_by: &'static [&'static str],
    /// The subcommands that ARE the interactive run, on something saved. A
    /// run naming any other subcommand the binary itself lists
    /// ([`TerminalArgsAnswer::Knows`]) is left exactly as it was started; a
    /// run naming none is the interactive one, bare or with a prompt.
    pub interactive_commands: &'static [&'static str],
}

/// How long [`with_terminal_args`] waits for an agent to say whether it
/// knows its words. codex-cli 0.166.0 answered `--version` in 60 ms and
/// 0.167.0 in 90 to 320 ms; an agent that has not answered by then starts as
/// it was typed.
pub const TERMINAL_ARGS_PROBE_LIMIT: std::time::Duration = std::time::Duration::from_secs(2);

/// The hook event a mirror sends through the window's bridge when an
/// interactive run that needed its terminal words starts without them, so
/// the window's log says so (t-97498). Its payload is a
/// [`TerminalArgsMissedNote`].
pub const TERMINAL_ARGS_MISSED_EVENT: &str = "ZerocodeTerminalArgsMissed";

/// What one binary said when it was asked about its terminal words:
/// `program <words> --help`. Asked once per binary and kept
/// ([`with_terminal_args`]), because the answer is the binary's and does not
/// change until the binary does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalArgsAnswer {
    /// It took the words, and its own help lists these subcommands and
    /// options ([`HelpWords::of`]).
    Knows(HelpWords),
    /// It refused the words with the argument parser's own exit code 2: a
    /// codex older than 0.156.0, which has no shared daemon and so needs
    /// none — or one that dropped the flag, which is why it is said once
    /// ([`TerminalArgsMissed::Refused`]). A CLI that parses its arguments
    /// before acting on `--help` refuses a word it does not know (exit 2,
    /// measured on codex-cli 0.166.0 and 0.167.0). Any other failure is not
    /// this answer and is not kept.
    Refused,
    /// It took the words but its help lists no subcommand, so a prompt cannot
    /// be told from one.
    Unreadable,
}

/// Why an interactive run that needed its terminal words starts without
/// them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalArgsMissed {
    /// [`TerminalArgsAnswer::Unreadable`].
    Unreadable,
    /// [`TerminalArgsAnswer::Refused`], said by the start that asked and by
    /// no later one: a build older than the shared daemon needs no words,
    /// but a build that dropped the flag and kept the daemon refuses the
    /// same way (t-98207 X3).
    Refused,
    /// It ended with a failure other than the argument parser's refusal (an
    /// update half written, a signal). Not kept: the next start asks again.
    Failed,
    /// It did not answer within the limit. Not kept: the next start asks
    /// again.
    NoAnswer,
}

/// What [`with_terminal_args`] did to one run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalArgsOutcome {
    /// The words went in front.
    Added,
    /// The run needs none: an agent without words, a run that already chose
    /// its server, a probe, a subcommand other than the interactive ones, or
    /// a binary whose refusal ([`TerminalArgsAnswer::Refused`]) this window
    /// was already told of.
    NotNeeded,
    /// The binary could not even be started to be asked (a file without its
    /// execute bit; on Windows an install that left only npm's extensionless
    /// `codex` for sh, which Windows cannot start). The run starts as typed,
    /// nothing is kept and nothing is said: there is no answer to report.
    NotAsked,
    /// The run starts exactly as it was asked, and this is what the window's
    /// log is told ([`TERMINAL_ARGS_MISSED_EVENT`]).
    Missed(TerminalArgsMissedNote),
}

/// The payload of [`TERMINAL_ARGS_MISSED_EVENT`]: which agent, which binary,
/// and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TerminalArgsMissedNote {
    /// The catalogue id, one [`AGENT_TERMINAL_ARGS`] names.
    pub agent: String,
    /// The binary the mirror stands in front of, as it was handed over.
    pub binary: String,
    /// Why.
    pub missed: TerminalArgsMissed,
}

impl TerminalArgsMissedNote {
    /// The note as the bridge carries it.
    #[must_use]
    pub fn payload(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The one line the window's log says for a payload, or `None` for one
    /// that is not a note about an agent [`AGENT_TERMINAL_ARGS`] names. The
    /// bridge is open to anything local, so the line is this table's own
    /// words around the binary's path, with no control character from it.
    #[must_use]
    pub fn line_of(payload: &str) -> Option<String> {
        let note: Self = serde_json::from_str(payload).ok()?;
        let rule = rule_of(&note.agent)?;
        let binary: String = note.binary.chars().filter(|ch| !ch.is_control()).collect();
        let why = match note.missed {
            TerminalArgsMissed::Unreadable => {
                "its help lists no subcommand, so a prompt cannot be told from one".to_string()
            }
            TerminalArgsMissed::Refused => {
                return Some(format!(
                    "{} at {binary} started without `{}`: it refused `{} --help` with exit 2, \
                     as a build older than the shared daemon does, and this window says so \
                     once — if it has the daemon, the commands it runs may wear another \
                     pane's identity",
                    rule.agent, rule.args, rule.args
                ));
            }
            TerminalArgsMissed::Failed => format!("`{} --help` failed", rule.args),
            TerminalArgsMissed::NoAnswer => format!(
                "it did not answer `{} --help` within {} ms",
                rule.args,
                TERMINAL_ARGS_PROBE_LIMIT.as_millis()
            ),
        };
        Some(format!(
            "{} at {binary} started without `{}`: {why} — the commands it runs may wear \
             another pane's identity",
            rule.agent, rule.args
        ))
    }
}

fn rule_of(agent: &str) -> Option<&'static TerminalArgs> {
    AGENT_TERMINAL_ARGS.iter().find(|rule| rule.agent == agent)
}

impl TerminalArgs {
    fn words(&self) -> Vec<String> {
        self.args.split_whitespace().map(str::to_string).collect()
    }

    /// Whether a run is left exactly as it was started before its binary is
    /// asked anything: it already chose its server, or it only asks the
    /// binary about itself.
    fn leaves_alone<S: AsRef<str>>(&self, argv: &[S]) -> bool {
        before_data(argv).any(|word| {
            self.chosen_by.iter().any(|flag| {
                word.strip_prefix(flag)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('='))
            })
        }) || crate::hook::is_agent_probe_argv(argv)
    }
}

/// The words of a run before a bare `--`: everything after it is data,
/// whatever it is spelled like.
fn before_data<S: AsRef<str>>(argv: &[S]) -> impl Iterator<Item = &str> {
    argv.iter()
        .map(AsRef::as_ref)
        .take_while(|word| *word != "--")
}

/// The words [`AGENT_TERMINAL_ARGS`] puts in front of one interactive run of
/// `agent` with these arguments, given the subcommands its binary lists —
/// empty when the agent has none, when the run already chose its server,
/// when it only asks the binary about itself, and when it names a subcommand
/// other than the interactive ones.
///
/// An allow list, so a subcommand codex adds tomorrow starts as typed until
/// someone has run it with the words. Which word IS a subcommand is the
/// binary's own list, not one kept here, and it is read where the binary's
/// parser reads one: the first word that is neither an option nor an
/// option's value ([`HelpWords::first_positional`]), so `-C app` or
/// `-p review` is a folder and a profile, not a subcommand.
#[must_use]
pub fn terminal_args<S: AsRef<str>>(agent: &str, argv: &[S], help: &HelpWords) -> Vec<String> {
    let Some(rule) = rule_of(agent) else {
        return Vec::new();
    };
    let other_command = help.first_positional(argv).is_some_and(|word| {
        !rule.interactive_commands.contains(&word) && help.commands.iter().any(|said| said == word)
    });
    if other_command || rule.leaves_alone(argv) {
        return Vec::new();
    }
    rule.words()
}

/// The subcommand names a clap `--help` lists under `Commands:`, each with
/// the aliases it shows (`exec  Run Codex non-interactively [aliases: e]`).
/// Empty when there is no such list.
#[must_use]
pub fn help_commands(help: &str) -> Vec<String> {
    const ALIAS_MARKS: [&str; 2] = ["[aliases: ", "[alias: "];
    let mut names = Vec::new();
    let mut entry_indent = None;
    for line in help
        .lines()
        .skip_while(|line| line.trim_end() != "Commands:")
        .skip(1)
    {
        let text = line.trim_start();
        // The list ends at a blank line or the next heading.
        if text.is_empty() || text.len() == line.len() {
            break;
        }
        // An entry starts at the list's own indent; a deeper line goes on
        // with the one above it.
        let indent = line.len() - text.len();
        if *entry_indent.get_or_insert(indent) == indent
            && let Some(name) = text.split_whitespace().next()
        {
            names.push(name.to_string());
        }
        for mark in ALIAS_MARKS {
            if let Some((_, rest)) = text.split_once(mark)
                && let Some((aliases, _)) = rest.split_once(']')
            {
                names.extend(
                    aliases
                        .split(',')
                        .map(str::trim)
                        .filter(|alias| !alias.is_empty())
                        .map(str::to_string),
                );
            }
        }
    }
    names
}

/// What a binary's own `--help` says about how its arguments are read: the
/// subcommands it lists and the options that take a value.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HelpWords {
    /// [`help_commands`].
    pub commands: Vec<String>,
    /// [`help_valued_options`].
    pub valued: Vec<ValuedOption>,
}

/// One option a clap `--help` lists with a value: `-C, --cd <DIR>` is two of
/// these, `-i, --image <FILE>...` two that take every value up to the next
/// option, and `--foo [<X>]` one whose value may be left out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValuedOption {
    /// The option as typed, `-C` or `--cd`.
    pub name: String,
    /// Whether it takes every following word up to the next option.
    pub many: bool,
    /// Whether its value may be left out (`[<VALUE>]`): then a next word that
    /// starts with `-` is the next option, as clap reads it, not this one's
    /// value (t-97731 P3-3).
    pub optional: bool,
}

impl HelpWords {
    /// What `help` says ([`help_commands`], [`help_valued_options`]).
    #[must_use]
    pub fn of(help: &str) -> Self {
        Self {
            commands: help_commands(help),
            valued: help_valued_options(help),
        }
    }

    fn valued(&self, name: &str) -> Option<&ValuedOption> {
        self.valued.iter().find(|option| option.name == name)
    }

    /// The first word of a run, before a bare `--`, that is neither an
    /// option nor an option's value — where clap reads a subcommand. A
    /// `--long=value` and a `-Xvalue` hold their own value; a valued option
    /// spelled alone takes the next word, or every word up to the next
    /// option when it takes many, and one whose value may be left out takes
    /// the next word only when it is not an option; `-abc` is `-a -b -c`
    /// until one of them takes a value.
    #[must_use]
    pub fn first_positional<'a, S: AsRef<str>>(&self, argv: &'a [S]) -> Option<&'a str> {
        let mut words = before_data(argv).peekable();
        while let Some(word) = words.next() {
            let takes = if word.starts_with("--") {
                if word.contains('=') {
                    None
                } else {
                    self.valued(word)
                }
            } else if let Some(shorts) = word.strip_prefix('-').filter(|rest| !rest.is_empty()) {
                shorts
                    .char_indices()
                    .find_map(|(at, short)| {
                        self.valued(&format!("-{short}"))
                            .map(|option| (at + short.len_utf8() == shorts.len(), option))
                    })
                    .and_then(|(last, option)| last.then_some(option))
            } else {
                return Some(word);
            };
            match takes {
                Some(option) if option.many => {
                    while words.next_if(|next| !next.starts_with('-')).is_some() {}
                }
                Some(option) if option.optional => {
                    words.next_if(|next| !next.starts_with('-'));
                }
                Some(_) => {
                    words.next();
                }
                None => {}
            }
        }
        None
    }
}

/// The options a clap `--help` lists under `Options:` with a value
/// (`<VALUE>`, or `[<VALUE>]`, which clap fills from the next word unless
/// that word is an option), each spelling apart. Empty when there is no
/// such list.
#[must_use]
pub fn help_valued_options(help: &str) -> Vec<ValuedOption> {
    let is_option = |word: &str| {
        word.strip_prefix("--")
            .or_else(|| word.strip_prefix('-'))
            .and_then(|rest| rest.chars().next())
            .is_some_and(|first| first.is_ascii_alphanumeric())
    };
    let mut valued = Vec::new();
    for line in help
        .lines()
        .skip_while(|line| line.trim_end() != "Options:")
        .skip(1)
    {
        let text = line.trim_start();
        // A long help puts a blank line between entries; the list ends at
        // the next heading.
        if text.is_empty() {
            continue;
        }
        if text.len() == line.len() {
            break;
        }
        let mut names = Vec::new();
        for word in text.split_whitespace() {
            let word = word.trim_end_matches(',');
            if is_option(word) {
                names.push(word.to_string());
                continue;
            }
            let optional = word.starts_with("[<");
            if word.starts_with('<') || optional {
                let many = word.ends_with("...");
                valued.extend(names.drain(..).map(|name| ValuedOption {
                    name,
                    many,
                    optional,
                }));
            }
            break;
        }
    }
    valued
}

/// The arguments an interactive run of `program` starts with — its own, with
/// the [`terminal_args`] of the agent it is in front — and what was done.
///
/// The binary is asked once, `program <words> --help` within `limit`: whether
/// it takes the words, and which subcommands and valued options it lists.
/// The answer is kept in `kept_in` (the mirror directory) under the binary's
/// path, modification time and size, so every later start of the same binary
/// reads it instead of asking; a binary replaced in place is asked again.
/// Only a real answer is kept — a list, a help without one, or the argument
/// parser's refusal (exit 2); a failure of any other kind, no answer in time
/// and a binary that could not be started are asked again next time. A
/// refusal, a help without a list, a failure, no answer, a binary that could
/// not be started and a binary this table does not name: the run starts
/// exactly as it was asked. A refusal is said once, by the start that asked
/// ([`TerminalArgsMissed::Refused`]).
///
/// Windows: the mirror is handed the file Windows starts
/// ([`crate::agent::resolve_on_path`]) — npm's `codex.cmd`, which std runs
/// through `cmd.exe`, or a native `codex.exe` — and it is found here by the
/// name it is called by ([`crate::agent::command_name_of`]). A question not
/// answered in time is ended with the `cmd.exe`, node and codex under it
/// ([`crate::process_tree::end_tree`], t-98207 X1).
#[must_use]
pub fn with_terminal_args(
    program: &std::path::Path,
    argv: Vec<std::ffi::OsString>,
    limit: std::time::Duration,
    kept_in: Option<&std::path::Path>,
) -> (Vec<std::ffi::OsString>, TerminalArgsOutcome) {
    let Some(rule) = crate::agent::command_name_of(program)
        .and_then(|name| {
            crate::AGENT_SPECS
                .iter()
                .find(|spec| spec.detect_names().any(|said| said == name))
        })
        .and_then(|spec| rule_of(spec.id))
    else {
        return (argv, TerminalArgsOutcome::NotNeeded);
    };
    let text = crate::hook::argv_text(&argv);
    if rule.leaves_alone(&text) {
        return (argv, TerminalArgsOutcome::NotNeeded);
    }
    let missed = |missed| {
        TerminalArgsOutcome::Missed(TerminalArgsMissedNote {
            agent: rule.agent.to_string(),
            binary: program.to_string_lossy().into_owned(),
            missed,
        })
    };
    let (asked, now) = answer_of(program, &rule.words(), limit, kept_in);
    let help = match asked {
        Asked::Answer(TerminalArgsAnswer::Knows(help)) => help,
        // Said by the start that asked, once per binary in this window:
        // every later start reads the kept refusal and says nothing. A build
        // older than the shared daemon needs no words, but one that dropped
        // the flag and kept the daemon refuses the same way (t-97731 P3-2).
        Asked::Answer(TerminalArgsAnswer::Refused) if now => {
            return (argv, missed(TerminalArgsMissed::Refused));
        }
        Asked::Answer(TerminalArgsAnswer::Refused) => {
            return (argv, TerminalArgsOutcome::NotNeeded);
        }
        Asked::Answer(TerminalArgsAnswer::Unreadable) => {
            return (argv, missed(TerminalArgsMissed::Unreadable));
        }
        Asked::Failed => return (argv, missed(TerminalArgsMissed::Failed)),
        Asked::NoAnswer => return (argv, missed(TerminalArgsMissed::NoAnswer)),
        Asked::NotStarted => return (argv, TerminalArgsOutcome::NotAsked),
    };
    let words = terminal_args(rule.agent, &text, &help);
    if words.is_empty() {
        return (argv, TerminalArgsOutcome::NotNeeded);
    }
    let argv = words
        .into_iter()
        .map(std::ffi::OsString::from)
        .chain(argv)
        .collect();
    (argv, TerminalArgsOutcome::Added)
}

/// What came of asking a binary about its terminal words. Only an
/// [`Asked::Answer`] is kept.
#[derive(Debug)]
enum Asked {
    Answer(TerminalArgsAnswer),
    /// It ended with a failure other than exit 2.
    Failed,
    /// It did not end within the limit.
    NoAnswer,
    /// It could not be started.
    NotStarted,
}

/// The kept answer of `program` about `words`, or a fresh one, kept — and
/// whether it was asked now (`true`) rather than read.
fn answer_of(
    program: &std::path::Path,
    words: &[String],
    limit: std::time::Duration,
    kept_in: Option<&std::path::Path>,
) -> (Asked, bool) {
    let kept = kept_in.and_then(|dir| answer_file(dir, program, words));
    if let Some((file, key)) = &kept
        && let Some(answer) = kept_answer(file, key)
    {
        return (Asked::Answer(answer), false);
    }
    let asked = ask_terminal_args(program, words, limit);
    if let (Some((file, key)), Asked::Answer(answer)) = (&kept, &asked) {
        keep_answer(file, key, answer);
    }
    (asked, true)
}

/// The shape [`keep_answer`] writes, first in every key it keeps an answer
/// under (t-98207 X2). The mirror folder is the window's, named by its pid,
/// and nothing removes it, so a window that is given an older window's pid
/// finds that window's files. A key without this mark is one of the first
/// two shapes (subcommands alone, t-97498; with valued options, t-97731),
/// and a file under any other mark is asked again rather than read in a
/// shape it was not written in. It changes whenever the shape does.
const KEPT_ANSWER_FORMAT: &str = "v3";

/// What follows a kept valued option's name when it takes every value up
/// to the next option ([`ValuedOption::many`]).
const KEPT_MANY: &str = "...";

/// What follows a kept valued option's name when its value may be left out
/// ([`ValuedOption::optional`]); before [`KEPT_MANY`] when it is both.
const KEPT_OPTIONAL: &str = "?";

/// Where the answer of `program` about `words` is kept in `dir`, and the key
/// it is kept under: the shape it is kept in ([`KEPT_ANSWER_FORMAT`]), the
/// binary's path, modification time and size (of the file the path leads
/// to — npm's link leads to a script it rewrites on every update) and the
/// words asked.
fn answer_file(
    dir: &std::path::Path,
    program: &std::path::Path,
    words: &[String],
) -> Option<(std::path::PathBuf, String)> {
    let held = std::fs::metadata(program).ok()?;
    let modified = held
        .modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_nanos();
    let key = format!(
        "{KEPT_ANSWER_FORMAT}\t{}\t{modified}\t{}\t{}",
        program.to_string_lossy(),
        held.len(),
        words.join(" ")
    );
    let file = dir.join(format!(
        "terminal-args-{}",
        crate::jev::fingerprint_of(&key)
    ));
    Some((file, key))
}

/// The answer kept in `file`, when it was kept under `key`. Its first line
/// is the key, its second what the binary said, and the rest the
/// subcommands it listed and, each starting with `-` as no subcommand does,
/// its valued options, [`KEPT_OPTIONAL`] after one whose value may be left
/// out and [`KEPT_MANY`] after one that takes many.
fn kept_answer(file: &std::path::Path, key: &str) -> Option<TerminalArgsAnswer> {
    let text = std::fs::read_to_string(file).ok()?;
    let mut lines = text.lines();
    if lines.next()? != key {
        return None;
    }
    match lines.next()? {
        // A yes without a list could not tell a subcommand from a prompt; it
        // is never kept, so one found is asked again.
        "knows" => {
            let mut help = HelpWords::default();
            for line in lines {
                if line.starts_with('-') {
                    let (line, many) = line
                        .strip_suffix(KEPT_MANY)
                        .map_or((line, false), |name| (name, true));
                    let (name, optional) = line
                        .strip_suffix(KEPT_OPTIONAL)
                        .map_or((line, false), |name| (name, true));
                    help.valued.push(ValuedOption {
                        name: name.to_string(),
                        many,
                        optional,
                    });
                } else {
                    help.commands.push(line.to_string());
                }
            }
            (!help.commands.is_empty()).then_some(TerminalArgsAnswer::Knows(help))
        }
        "refused" => Some(TerminalArgsAnswer::Refused),
        "unreadable" => Some(TerminalArgsAnswer::Unreadable),
        _ => None,
    }
}

/// Keep `answer` in `file` under `key`, whole or not at all. A keep that
/// fails costs the next start one more question, nothing else.
fn keep_answer(file: &std::path::Path, key: &str, answer: &TerminalArgsAnswer) {
    let said = match answer {
        TerminalArgsAnswer::Knows(help) => std::iter::once("knows".to_string())
            .chain(help.commands.iter().cloned())
            .chain(help.valued.iter().map(|option| {
                format!(
                    "{}{}{}",
                    option.name,
                    if option.optional { KEPT_OPTIONAL } else { "" },
                    if option.many { KEPT_MANY } else { "" }
                )
            }))
            .collect::<Vec<_>>()
            .join("\n"),
        TerminalArgsAnswer::Refused => "refused".to_string(),
        TerminalArgsAnswer::Unreadable => "unreadable".to_string(),
    };
    let _ = crate::atomic_file::write_atomically(file, format!("{key}\n{said}\n").as_bytes());
}

/// The question [`ask_terminal_args`] asks: `program <words> --help`, its
/// help uncoloured whatever the person's shell exported, so a heading is
/// read as the words it is. It reads nothing from the terminal, and leads a
/// tree of its own ([`crate::process_tree::lead_own_tree`]): npm's codex is
/// a script whose binary runs under it (under `cmd.exe` and node on
/// Windows), and a question not answered in time is ended with all of it.
fn terminal_args_question(program: &std::path::Path, words: &[String]) -> std::process::Command {
    use std::process::{Command, Stdio};
    let mut question = Command::new(program);
    question
        .args(words)
        .arg("--help")
        .env("NO_COLOR", "1")
        .env_remove("CLICOLOR_FORCE")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    crate::process_tree::lead_own_tree(&mut question);
    question
}

/// Ask `program <words> --help` within `limit`. Ended rather than abandoned
/// when it does not end in time — the whole tree it started, so no part of a
/// question outlives the run it was asked for. Its one caller is the mirror,
/// a console program in the person's terminal, so the question shares that
/// console as the run does; only its answer is read.
fn ask_terminal_args(
    program: &std::path::Path,
    words: &[String],
    limit: std::time::Duration,
) -> Asked {
    use std::io::Read as _;
    let began = std::time::Instant::now();
    let Ok(mut child) = terminal_args_question(program, words).spawn() else {
        return Asked::NotStarted;
    };
    // Read beside the wait: a help longer than a pipe holds would stall the
    // binary until the limit. Received with what is left of the limit, so a
    // child that hands its output on to one that stays cannot hold the run.
    let (said, heard) = std::sync::mpsc::channel();
    if let Some(mut out) = child.stdout.take() {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let _ = out.read_to_end(&mut bytes);
            let _ = said.send(String::from_utf8_lossy(&bytes).into_owned());
        });
    }
    loop {
        match child.try_wait() {
            // The argument parser's own refusal; nothing else is one.
            Ok(Some(status)) if status.code() == Some(2) => {
                return Asked::Answer(TerminalArgsAnswer::Refused);
            }
            Ok(Some(status)) if !status.success() => return Asked::Failed,
            Ok(Some(_)) => {
                let Ok(help) = heard.recv_timeout(limit.saturating_sub(began.elapsed())) else {
                    return Asked::NoAnswer;
                };
                let help = HelpWords::of(&help);
                return Asked::Answer(if help.commands.is_empty() {
                    TerminalArgsAnswer::Unreadable
                } else {
                    TerminalArgsAnswer::Knows(help)
                });
            }
            Ok(None) if began.elapsed() < limit => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            _ => {
                crate::process_tree::end_tree(&mut child);
                return Asked::NoAnswer;
            }
        }
    }
}

/// The default arguments for one agent, or `""` for an agent with none.
pub fn default_launch_args(agent: &str) -> &'static str {
    AGENT_LAUNCH_ARGS
        .iter()
        .find(|(id, _)| *id == agent)
        .map(|(_, args)| *args)
        .unwrap_or("")
}

/// The default environment for one agent — empty for all but `goose`.
pub fn default_launch_env(agent: &str) -> &'static [(&'static str, &'static str)] {
    AGENT_LAUNCH_ENV
        .iter()
        .find(|(id, _)| *id == agent)
        .map(|(_, env)| *env)
        .unwrap_or(&[])
}

/// Does this agent have any way to be told not to ask?
///
/// Orca's `PERMISSION_AGENT_IDS` — the agents that appear in either table, and
/// the only ones whose permission mode is a question with an answer. For the
/// rest the window says nothing rather than implying a setting exists.
pub fn has_permission_switch(agent: &str) -> bool {
    !default_launch_args(agent).is_empty() || !default_launch_env(agent).is_empty()
}

/// Split one command-line string the way Orca's own tokenizer does
/// (`tokenizeCustomCommandTemplate`, commit-message-prompt.ts:164-264, the
/// escape mode `tokenizeStartupCommand` feeds agent args through).
///
/// The rules, exactly: quotes group and are removed — `a"b c"d` is ONE word
/// `ab cd`; a backslash takes the next character literally, inside double
/// quotes and out (single quotes are literal throughout); whitespace splits
/// only outside quotes. An unclosed quote is an error, not a guess — Orca
/// refuses the launch plan over it (`planAgentCliArgsSuffix`), because argv
/// built from a half-read string runs somebody's flags wrong.
///
/// This exists because `split_whitespace` handed `continue` a literal
/// two-byte-quoted `"*"` for `--allow`, and cut a person's
/// `--add-dir "path with spaces"` into three arguments — the map's P0-9.
pub fn split_command_line(line: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();
    while let Some(ch) = chars.next() {
        match quote {
            Some(mark) => {
                if ch == '\\' && mark == '"' {
                    // Inside double quotes the tokenizer consumes the
                    // backslash and keeps whatever follows — Orca does not
                    // reproduce the shell's narrower `$`/backtick list here,
                    // it only flags the divergence for a feature this
                    // splitter does not carry.
                    if let Some(next) = chars.next() {
                        current.push(next);
                        continue;
                    }
                    current.push(ch);
                } else if ch == mark {
                    quote = None;
                } else {
                    current.push(ch);
                }
            }
            None => {
                if ch == '"' || ch == '\'' {
                    quote = Some(ch);
                    in_word = true;
                } else if ch == '\\' {
                    match chars.next() {
                        Some(next) => current.push(next),
                        None => current.push(ch),
                    }
                    in_word = true;
                } else if ch.is_whitespace() {
                    if in_word {
                        words.push(std::mem::take(&mut current));
                        in_word = false;
                    }
                } else {
                    current.push(ch);
                    in_word = true;
                }
            }
        }
    }
    if quote.is_some() {
        return Err("Unclosed quote in command template.".to_string());
    }
    if in_word {
        words.push(current);
    }
    Ok(words)
}

/// Render argv as the command-line grammar [`split_command_line`] reads.
///
/// This is deliberately not POSIX shell quoting. Some launch roads hand the
/// rendered line directly to a shell, but the native PTY and orchestration
/// roads feed it back through our quote-aware tokenizer and then call
/// `Command` with argv. POSIX's `'<text>'\''<text>'` spelling for an apostrophe
/// is not part of that tokenizer's grammar; using it split one orchestration
/// briefing into dozens of Claude positional arguments and the worker exited
/// before its first turn.
#[must_use]
pub fn join_command_line(words: &[String]) -> String {
    words
        .iter()
        .map(|word| {
            let plain = !word.is_empty()
                && word
                    .chars()
                    .all(|ch| !ch.is_whitespace() && !matches!(ch, '\'' | '"' | '\\'));
            if plain {
                return word.clone();
            }
            let escaped = word.replace('\\', "\\\\").replace('"', "\\\"");
            format!("\"{escaped}\"")
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The one line of arguments a launch starts from — the override as written,
/// or the measured default. This is the string the settings row shows and
/// the person edits; handing them the SPLIT words re-joined would silently
/// strip the quotes that made `"path with spaces"` one argument.
pub fn launch_args_line(agent: &str, held: Option<&LaunchOverride>) -> String {
    match held.and_then(|one| one.args.as_deref()) {
        Some(written) => sanitize_launch_args(agent, written),
        None => default_launch_args(agent).to_string(),
    }
}

/// One agent's environment override as the LINE a person edits.
///
/// The twin of [`launch_args_line`], and it exists for the same reason: the
/// field is a line, and rendering it from the resolved pairs is what lets the
/// window show what is stored rather than what was computed. Absent means "no
/// opinion" and the measured default is shown; present-and-empty is an opinion
/// and shows as an empty line.
///
/// Values holding whitespace come back quoted, so the line this returns parses
/// into the pairs it came from — the round trip the settings field depends on.
pub fn launch_env_line(agent: &str, held: Option<&LaunchOverride>) -> String {
    let pairs: Vec<(String, String)> = match held.and_then(|one| one.env.as_ref()) {
        Some(written) => written.clone(),
        None => default_launch_env(agent)
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect(),
    };
    pairs
        .iter()
        .map(|(name, value)| format!("{name}={}", quote_if_needed(value)))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Take out the flags this agent refuses, leaving the rest **as written**.
///
/// Whole words only. `argPattern` anchors on whitespace or string edge for the
/// same reason: `--dangerously-skip-permissions-please` is somebody else's flag
/// and stripping its prefix would corrupt it into nonsense.
///
/// The filter runs on the quote-aware split, not on `split_whitespace`, and the
/// original string is returned untouched whenever nothing is removed — which is
/// every agent but two. The old `split_whitespace().join(" ")` collapsed a
/// person's `base_url="http://…/v1"` into three pieces the moment this ran,
/// the same P0-9 defect `split_command_line` was written to end. A caller now
/// puts quoted `-c key="value"` here for the cross-harness bridge, so the
/// collapse is no longer theoretical.
pub fn sanitize_launch_args(agent: &str, args: &str) -> String {
    let refused = AGENT_REFUSED_ARGS
        .iter()
        .find(|(id, _)| *id == agent)
        .map(|(_, list)| *list)
        .unwrap_or(&[]);
    if refused.is_empty() {
        return args.trim().to_string();
    }
    // Split the way a shell would, so a refused flag is matched as a whole
    // argument and a quoted value is one piece. If the split fails — an
    // unclosed quote — leave the string alone: the save door refuses it
    // separately with a message, and mangling it here would hide that.
    let Ok(words) = split_command_line(args) else {
        return args.trim().to_string();
    };
    // Nothing to take out means nothing to rewrite: hand back exactly what was
    // written, quotes and all.
    if !words.iter().any(|word| refused.contains(&word.as_str())) {
        return args.trim().to_string();
    }
    words
        .into_iter()
        .filter(|word| !refused.contains(&word.as_str()))
        .map(|word| quote_if_needed(&word))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Wrap a word in double quotes when it holds a space, so a rewritten line
/// splits back into the same arguments it came from.
fn quote_if_needed(word: &str) -> String {
    if word.is_empty() || word.chars().any(char::is_whitespace) {
        format!("\"{}\"", word.replace('"', "\\\""))
    } else {
        word.to_string()
    }
}

/// What a launch will be allowed to do, as one word.
///
/// Orca derives this to decide what its settings pane shows
/// (`resolveTuiAgentPermissionMode`): the configured value equal to the yolo
/// value is `Unattended`, empty is `Asks`, and anything else is `Mixed` —
/// somebody has edited it and only they know what it means now.
///
/// It is here rather than in the window because it is a *judgement about a
/// launch*, and the launch is assembled here. A window that derived it
/// separately would be a second opinion about the same command line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionMode {
    /// Nothing said — the agent will stop and ask, in a pane that may be
    /// behind another tab.
    Asks,
    /// The measured default: the agent proceeds without asking.
    Unattended,
    /// Edited to something that is neither, so the window does not claim to
    /// know which it is.
    Mixed,
}

/// The two global choices Orca exposes for agent permissions.
///
/// `Mixed` is deliberately absent: it is a derived summary for launch fields
/// somebody edited by hand, not a value a global control is allowed to write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentPermissionMode {
    Yolo,
    Manual,
}

impl PermissionMode {
    fn of_args(configured: &str, yolo: &str) -> Self {
        let said = configured.trim();
        if said.is_empty() {
            return Self::Asks;
        }
        if said == yolo {
            Self::Unattended
        } else {
            Self::Mixed
        }
    }

    fn of_env(configured: &[(String, String)], yolo: &[(&str, &str)]) -> Self {
        if configured.is_empty() {
            return Self::Asks;
        }
        let same = configured.len() == yolo.len()
            && yolo.iter().all(|(name, value)| {
                configured
                    .iter()
                    .any(|(held, set)| held == name && set == value)
            });
        if same { Self::Unattended } else { Self::Mixed }
    }

    /// One answer from several. Orca's `combinePermissionModes`: anything mixed
    /// makes the whole thing mixed, and so does unattended sitting beside asks
    /// — an agent half-told is not a state worth a confident word.
    fn combine(modes: &[Self]) -> Self {
        let mixed = modes.contains(&Self::Mixed);
        let unattended = modes.contains(&Self::Unattended);
        let asks = modes.contains(&Self::Asks);
        if mixed || (unattended && asks) {
            return Self::Mixed;
        }
        if unattended {
            Self::Unattended
        } else {
            Self::Asks
        }
    }
}

/// How an agent will be started: the words after the command, the variables
/// set for it, and what that adds up to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchPlan {
    /// Arguments, already split the way a process wants them.
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub permission: PermissionMode,
}

/// An override a person has typed for one agent. Both halves are optional
/// because the settings pane can carry either without the other.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<Vec<(String, String)>>,
}

/// The plan for launching `agent`, honouring an override where there is one.
///
/// Orca's `resolveTuiAgentLaunchArgs`/`Env`: the presence of a key is the
/// answer, not its truthiness. Somebody who has emptied the args field means
/// "ask me" and must not be silently given the default back — which is why the
/// override's fields are `Option` rather than strings that could be empty for
/// two different reasons.
pub fn launch_plan(agent: &str, held: Option<&LaunchOverride>) -> LaunchPlan {
    let yolo_args = default_launch_args(agent);
    let yolo_env = default_launch_env(agent);

    let args_said = launch_args_line(agent, held);
    let env_said: Vec<(String, String)> = match held.and_then(|one| one.env.as_ref()) {
        Some(written) => written.clone(),
        None => yolo_env
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect(),
    };

    let mut modes = Vec::new();
    if !yolo_args.is_empty() {
        modes.push(PermissionMode::of_args(&args_said, yolo_args));
    }
    if !yolo_env.is_empty() {
        modes.push(PermissionMode::of_env(&env_said, yolo_env));
    }

    // Split the way the strings were written to be read: `continue`'s own
    // measured default is `--allow "*"`, and whitespace alone handed the
    // agent a literal two-byte `"*"` — and cut a person's quoted path
    // into pieces (P0-9). An unclosed quote falls back to the old
    // whitespace split so a legacy override still launches; the save
    // door refuses to store a new one.
    let mut args = split_command_line(&args_said)
        .unwrap_or_else(|_| args_said.split_whitespace().map(str::to_string).collect());
    // After the person's line, whatever they wrote: the words that keep the
    // CLI starting here are the window's to add, not a field to edit.
    args.extend(compat_launch_args(agent));
    LaunchPlan {
        args,
        env: env_said,
        permission: PermissionMode::combine(&modes),
    }
}

/// Apply Orca's global Yolo/Manual choice to one agent override.
///
/// Only a permission field that is empty/manual or exactly the measured Yolo
/// value changes. A custom value is somebody's explicit launch contract and
/// stays byte-for-byte intact. Arguments and environment are handled
/// independently so changing one permission mechanism cannot erase an
/// unrelated override in the other half.
#[must_use]
pub fn apply_agent_permission_mode(
    agent: &str,
    held: Option<&LaunchOverride>,
    mode: AgentPermissionMode,
) -> Option<LaunchOverride> {
    let mut next = held.cloned().unwrap_or_default();
    let yolo_args = default_launch_args(agent);
    if !yolo_args.is_empty() {
        let current = next
            .args
            .as_deref()
            .map(|written| sanitize_launch_args(agent, written))
            .unwrap_or_else(|| yolo_args.to_string());
        if current.is_empty() || current == yolo_args {
            next.args = match mode {
                AgentPermissionMode::Yolo => None,
                AgentPermissionMode::Manual => Some(String::new()),
            };
        }
    }

    let yolo_env = default_launch_env(agent);
    if !yolo_env.is_empty() {
        let current = next.env.clone().unwrap_or_else(|| {
            yolo_env
                .iter()
                .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
                .collect()
        });
        let is_yolo = PermissionMode::of_env(&current, yolo_env) == PermissionMode::Unattended;
        if current.is_empty() || is_yolo {
            next.env = match mode {
                AgentPermissionMode::Yolo => None,
                AgentPermissionMode::Manual => Some(Vec::new()),
            };
        }
    }

    (next.args.is_some() || next.env.is_some()).then_some(next)
}

#[cfg(test)]
mod tests {
    /// How long a fake codex that answers is given in these tests (gate l108d,
    /// v1.1.128): a freshly written script's first start is checked by the
    /// system, and under a gate's parallel load that took past the product's
    /// TERMINAL_ARGS_PROBE_LIMIT (2 s) — the binary answered, the test read
    /// NoAnswer. A binary meant to stay silent is still given the real limit.
    /// Only the unix tests start a fake script, so the constant carries their cfg.
    #[cfg(unix)]
    const ANSWERING_LIMIT: std::time::Duration = std::time::Duration::from_secs(15);

    use super::*;
    use crate::AGENT_SPECS;

    #[test]
    fn every_agent_named_here_is_an_agent_this_window_knows() {
        // A typo in one of these tables is an agent that silently launches
        // without its flag — the exact failure this module exists to prevent,
        // arriving from the other direction.
        for (id, _) in AGENT_LAUNCH_ARGS {
            assert!(
                AGENT_SPECS.iter().any(|spec| spec.id == *id),
                "`{id}` has launch args but is not in the catalogue"
            );
        }
        for (id, _) in AGENT_LAUNCH_ENV {
            assert!(
                AGENT_SPECS.iter().any(|spec| spec.id == *id),
                "`{id}` has launch env but is not in the catalogue"
            );
        }
        for (id, _) in AGENT_REFUSED_ARGS {
            assert!(
                AGENT_SPECS.iter().any(|spec| spec.id == *id),
                "`{id}` refuses a flag but is not in the catalogue"
            );
        }
        for (id, words) in AGENT_LAUNCH_COMPAT_ARGS {
            assert!(
                AGENT_SPECS.iter().any(|spec| spec.id == *id),
                "`{id}` has compatibility words but is not in the catalogue"
            );
            // The words split cleanly and are nobody's refused flag.
            let split = split_command_line(words).expect("compatibility words split");
            assert_eq!(compat_launch_args(id), split);
            assert!(!split.iter().any(|word| {
                AGENT_REFUSED_ARGS
                    .iter()
                    .any(|(_, refused)| refused.contains(&word.as_str()))
            }));
        }
        for rule in AGENT_TERMINAL_ARGS {
            assert!(
                AGENT_SPECS.iter().any(|spec| spec.id == rule.agent),
                "`{}` has terminal words but is not in the catalogue",
                rule.agent
            );
            // A run that already says the words is one that chose its server.
            let split = split_command_line(rule.args).expect("terminal words split");
            assert!(!split.is_empty());
            for word in &split {
                assert!(rule.chosen_by.contains(&word.as_str()), "{word}");
            }
        }
    }

    /// codex-cli 0.167.0's own `codex --no-daemon --help`, as it printed into
    /// a pipe with `NO_COLOR=1` (trailing blanks trimmed): what
    /// [`HelpWords::of`] reads.
    const CODEX_0_167_HELP: &str = r#"Codex CLI

If no subcommand is specified, options will be forwarded to the interactive CLI.

Usage: codex [OPTIONS] [PROMPT]
       codex [OPTIONS] <COMMAND> [ARGS]

Commands:
  agents            Open the agent command center to browse and switch sessions
  exec              Run Codex non-interactively [aliases: e]
  review            Run a code review non-interactively
  login             Manage login
  logout            Remove stored authentication credentials
  mcp               Manage external MCP servers for Codex
  plugin            Manage Codex plugins
  app-server        [experimental] Run the app server or related tooling
  remote-control    [experimental] Manage the app-server daemon with remote control enabled
  app               Launch the Desktop app (opens the app installer if missing)
  completion        Generate shell completion scripts
  update            Update Codex to the latest version
  doctor            Diagnose local Codex installation, config, auth, and runtime health
  sandbox           Run commands within a Codex-provided sandbox
  debug             Debugging tools
  apply             Apply the latest diff produced by Codex agent as a `git apply` to your local
                    working tree [aliases: a]
  resume            Resume a previous interactive session (picker by default; use --last to continue
                    the most recent)
  queue             Queue a message for an existing session
  archive           Archive a saved session by id or session name
  delete            Permanently delete a saved session by id or session name
  migrate-rollouts  Inspect or migrate legacy local sessions to paginated thread history
  migrate-nfs-db    [experimental] Migrate configured SQLite databases to DELETE mode in place
  unarchive         Unarchive a saved session by id or session name
  fork              Fork a previous interactive session (picker by default; use --last to fork the
                    most recent)
  cloud             [EXPERIMENTAL] Browse tasks from Codex Cloud and apply changes locally
  exec-server       [EXPERIMENTAL] Run the standalone exec-server service
  features          Inspect feature flags
  help              Print this message or the help of the given subcommand(s)

Arguments:
  [PROMPT]
          Optional user prompt to start the session

Options:
  -c, --config <key=value>
          Override a configuration value that would otherwise be loaded from `~/.codex/config.toml`.
          Use a dotted path (`foo.bar.baz`) to override nested values. The `value` portion is parsed
          as TOML. If it fails to parse as TOML, the raw string is used as a literal.

          Examples: - `-c model="o3"` - `-c 'sandbox_permissions=["disk-full-read-access"]'` - `-c
          shell_environment_policy.inherit=all`

      --enable <FEATURE>
          Enable a feature (repeatable). Equivalent to `-c features.<name>=true`

      --disable <FEATURE>
          Disable a feature (repeatable). Equivalent to `-c features.<name>=false`

      --remote <ADDR>
          Connect the TUI to a remote app server endpoint.

          Accepted forms: `ws://host:port`, `wss://host:port`, `unix://`, or `unix://PATH`.

      --remote-auth-token-env <ENV_VAR>
          Name of the environment variable containing the bearer token to send to a remote app
          server websocket

      --strict-config
          Error out when config.toml contains fields that are not recognized by this version of
          Codex

  -i, --image <FILE>...
          Optional image(s) to attach to the initial prompt

  -m, --model <MODEL>
          Model the agent should use

      --oss
          Use open-source provider

      --local-provider <OSS_PROVIDER>
          Specify which local provider to use (lmstudio or ollama). If not specified with --oss,
          will use config default or show selection

  -p, --profile <CONFIG_PROFILE_V2>
          Layer $CODEX_HOME/<name>.config.toml on top of the base user config

  -s, --sandbox <SANDBOX_MODE>
          Select the sandbox policy to use when executing model-generated shell commands

          [possible values: read-only, workspace-write, danger-full-access]

      --approve-for-me
          Route approval requests through automatic review using the workspace-write sandbox

      --dangerously-bypass-approvals-and-sandbox
          Skip all confirmation prompts and execute commands without sandboxing. EXTREMELY
          DANGEROUS. Intended solely for running in environments that are externally sandboxed

      --dangerously-bypass-hook-trust
          Run enabled hooks without requiring persisted hook trust for this invocation. DANGEROUS.
          Intended only for automation that already vets hook sources

  -C, --cd <DIR>
          Tell the agent to use the specified directory as its working root

      --worktree
          Run the session in a new managed Git worktree

      --add-dir <DIR>
          Additional directories that should be writable alongside the primary workspace

  -a, --ask-for-approval <APPROVAL_POLICY>
          Configure when the model requires human approval before executing a command

          Possible values:
          - on-request: The model decides when to ask the user for approval
          - never:      Never ask for user approval Execution failures are immediately returned to
            the model

      --search
          Enable live web search. When enabled, the native Responses `web_search` tool is available
          to the model (no per‑call approval)

      --no-alt-screen
          Disable alternate screen mode

          Runs the TUI in inline mode, preserving terminal scrollback history.

      --no-daemon
          Run without the shared background server, even if it is already running

  -h, --help
          Print help (see a summary with '-h')

  -V, --version
          Print version
"#;

    /// Every subcommand codex lists, and every alias it shows, is read from
    /// its own help; a wrapped description adds no name.
    #[test]
    fn a_binarys_subcommands_are_read_from_its_own_help() {
        let commands = help_commands(CODEX_0_167_HELP);
        for listed in [
            "agents",
            "exec",
            "e",
            "login",
            "mcp",
            "apply",
            "a",
            "resume",
            "fork",
            "queue",
            "remote-control",
            "migrate-rollouts",
            "help",
        ] {
            assert!(commands.iter().any(|said| said == listed), "{listed}");
        }
        assert_eq!(commands.len(), 30, "{commands:?}");
        for not_one in ["working", "the", "most", "[PROMPT]", "Arguments:", "-c"] {
            assert!(!commands.iter().any(|said| said == not_one), "{not_one}");
        }
        assert!(help_commands("codex-cli 0.167.0\n").is_empty());
        assert!(help_commands("").is_empty());
    }

    /// Every option codex lists with a value is read from the same help, by
    /// each spelling, and one that takes many says so; an option without a
    /// value and a description line are not one (t-97498 P2-A).
    #[test]
    fn a_binarys_valued_options_are_read_from_its_own_help() {
        let valued = help_valued_options(CODEX_0_167_HELP);
        let named = |name: &str| valued.iter().find(|option| option.name == name);
        for (name, many) in [
            ("-c", false),
            ("--config", false),
            ("--enable", false),
            ("--remote", false),
            ("-i", true),
            ("--image", true),
            ("-m", false),
            ("--local-provider", false),
            ("-p", false),
            ("--profile", false),
            ("-s", false),
            ("-C", false),
            ("--cd", false),
            ("--add-dir", false),
            ("-a", false),
            ("--ask-for-approval", false),
        ] {
            assert_eq!(
                named(name).map(|option| option.many),
                Some(many),
                "{name}: {valued:?}"
            );
        }
        assert_eq!(valued.len(), 20, "{valued:?}");
        for not_one in [
            "--oss",
            "--worktree",
            "--search",
            "--no-daemon",
            "-h",
            "--help",
            "-V",
            "-",
            "on-request:",
        ] {
            assert!(named(not_one).is_none(), "{not_one}");
        }
        // codex-cli 0.167.0 has no option whose value may be left out; one
        // printed `[<X>]` is read as one (t-97731 P3-3).
        assert!(valued.iter().all(|option| !option.optional), "{valued:?}");
        let foo = help_valued_options(&help_with_an_optional_value());
        assert_eq!(
            foo.iter().find(|option| option.name == "--foo"),
            Some(&ValuedOption {
                name: "--foo".to_string(),
                many: false,
                optional: true,
            })
        );
        assert!(help_valued_options("Usage: codex\n").is_empty());
        assert!(HelpWords::of("").valued.is_empty());
    }

    /// The words every interactive `codex` in a ZeroCode terminal wears, and
    /// the runs that are left exactly as they were started (t-97015, t-97498).
    ///
    /// An allow list: the TUI — bare, with a prompt or with flags — and
    /// `resume` and `fork`, which are the TUI on a saved conversation. Every
    /// other subcommand the binary lists starts as typed, `exec`, `login`,
    /// `mcp` and `apply` among them: `codex --no-daemon exec --help` ending 0
    /// says nothing of the run, because `--help` ends before it.
    #[test]
    fn an_interactive_codex_keeps_its_commands_in_its_own_terminal() {
        let help = HelpWords::of(CODEX_0_167_HELP);
        let words = |argv: &[&str]| terminal_args("codex", argv, &help);
        let kept = ["--no-daemon"];

        // Typed bare, with a prompt, with flags that take values, resumed or
        // forked: the commands run in this terminal.
        assert_eq!(words(&[]), kept);
        assert_eq!(words(&["fix the build"]), kept);
        assert_eq!(
            words(&["-m", "gpt-6-astra", "-c", "model_reasoning_effort=high"]),
            kept
        );
        assert_eq!(words(&["resume", "019e7d20"]), kept);
        assert_eq!(words(&["resume", "--last"]), kept);
        assert_eq!(words(&["fork", "019e7d20"]), kept);
        // The window's own launch wears them too — its plan is unchanged and
        // the words are added in front of it, not into it.
        assert_eq!(
            terminal_args("codex", &launch_plan("codex", None).args, &help),
            kept
        );
        // A prompt that merely names a subcommand is one word, not that one.
        assert_eq!(words(&["explain codex agents"]), kept);
        // After a bare `--` everything is data.
        assert_eq!(words(&["--", "--remote"]), kept);
        assert_eq!(words(&["--", "exec"]), kept);
        // An option's value spelled like a subcommand is that option's: a
        // folder called app, a profile called review, an image called cloud
        // (t-97498 P2-A). Only the first word that is neither an option nor
        // a value is read as a subcommand, as clap reads one.
        for valued in [
            &["-C", "app"][..],
            &["-C", "app", "fix the build"],
            &["--cd", "app"],
            &["-p", "review"],
            &["--profile", "review"],
            &["--add-dir", "cloud", "-m", "gpt-6-astra"],
            &["-c", "sandbox"],
            &["-i", "a.png", "cloud", "debug"],
            &["--cd=app"],
            &["-Capp"],
            &["--search", "-C", "app"],
            &["resume", "--last", "review"],
        ] {
            assert_eq!(words(valued), kept, "{valued:?}");
        }

        // Every other subcommand, by name or by alias, starts as typed.
        for other in [
            &["exec", "summarize"][..],
            &["e", "summarize"],
            &["-m", "gpt-6-astra", "exec", "summarize"],
            &["exec", "resume", "--last"],
            &["-C", "app", "exec", "summarize"],
            &["--cd=app", "review"],
            &["-Capp", "login"],
            &["-i", "a.png", "-m", "gpt-6-astra", "exec"],
            &["--search", "review"],
            &["login"],
            &["login", "status"],
            &["mcp", "list"],
            &["apply"],
            &["a"],
            &["review"],
        ] {
            assert!(words(other).is_empty(), "{other:?}");
        }

        // A run that already chose its server: the words themselves, or a
        // server named either way (codex refuses `--no-daemon` beside it).
        for chosen in [
            &["--no-daemon"][..],
            &["--remote", "ws://192.0.2.10:4500"],
            &["--remote=unix:///tmp/zc.sock"],
            &["resume", "--remote", "unix:///tmp/zc.sock", "019e7d20"],
        ] {
            assert!(words(chosen).is_empty(), "{chosen:?}");
        }
        // A run about the shared server itself.
        for server in [
            &["agents"][..],
            &["queue", "--thread", "019e7d20", "--message", "go on"],
            &["archive", "019e7d20"],
            &["unarchive", "019e7d20"],
            &["delete", "019e7d20"],
            &["remote-control"],
            &["remote-control", "status"],
        ] {
            assert!(words(server).is_empty(), "{server:?}");
        }
        // A probe asks the binary about itself and is left exact.
        for probe in [&["--version"][..], &["-h"], &["exec", "--help"]] {
            assert!(words(probe).is_empty(), "{probe:?}");
        }
        // Which word is a subcommand is the binary's own list: one codex
        // adds tomorrow starts as typed the day it ships.
        assert_eq!(words(&["tomorrow"]), kept);
        let mut tomorrow = help.clone();
        tomorrow.commands.push("tomorrow".to_string());
        assert!(terminal_args("codex", &["tomorrow"], &tomorrow).is_empty());
        // Nobody else is handed codex's words.
        assert!(terminal_args("claude", &[] as &[&str], &help).is_empty());
        assert!(terminal_args("zo", &[] as &[&str], &help).is_empty());
    }

    /// codex-cli 0.167.0's help with one more option whose value may be left
    /// out, the shape clap prints for `num_args(0..=1)`.
    fn help_with_an_optional_value() -> String {
        CODEX_0_167_HELP.replacen(
            "Options:\n",
            "Options:\n      --foo [<X>]\n          A value that may be left out\n\n",
            1,
        )
    }

    /// An option whose value may be left out takes the next word only when
    /// that word is not an option, as clap reads it (t-97731 P3-3): in
    /// `--foo -C app "fix it"` the `-C` is the next option and `app` its
    /// folder, not `--foo`'s value and a subcommand — which started the TUI
    /// without its words.
    #[test]
    fn an_optional_value_never_takes_the_next_option() {
        let help = HelpWords::of(&help_with_an_optional_value());
        let words = |argv: &[&str]| terminal_args("codex", argv, &help);
        let kept = ["--no-daemon"];
        assert_eq!(words(&["--foo", "-C", "app", "fix it"]), kept);
        assert_eq!(words(&["--foo", "--cd", "app"]), kept);
        // A word that is not an option is still its value.
        assert_eq!(words(&["--foo", "review"]), kept);
        assert_eq!(words(&["--foo", "review", "fix it"]), kept);
        // And a subcommand after it is still read as one.
        assert!(words(&["--foo", "-C", "app", "exec", "go"]).is_empty());
        assert_eq!(words(&["--foo", "--", "-C"]), kept);
    }

    /// A question that is not answered in time is ended with everything it
    /// started (t-98207 X1). npm's codex is a script whose binary runs under
    /// it — on Windows under `cmd.exe` and node — and ending the script
    /// alone left the binary running. Here the script's child stands in for
    /// that binary, and the kernel is asked about it by its pid.
    #[cfg(unix)]
    #[test]
    fn a_question_not_answered_in_time_is_ended_with_what_it_started() {
        let dir = tempfile::tempdir().expect("a fake codex");
        let codex = fake_binary(
            dir.path(),
            "codex",
            r#"[ $# -eq 0 ] && exit 0
sleep 30 & echo $! > "$0.child"; wait"#,
        );
        // Started once with no words first, so the system's check of a
        // freshly written file is spent before the real limit runs and the
        // question reaches its child (see ANSWERING_LIMIT).
        assert!(
            std::process::Command::new(&codex)
                .status()
                .expect("the fake starts")
                .success()
        );
        assert_eq!(
            with_terminal_args(&codex, os_words(&[]), TERMINAL_ARGS_PROBE_LIMIT, None),
            (Vec::new(), missed(&codex, TerminalArgsMissed::NoAnswer))
        );
        let child: u32 = std::fs::read_to_string(dir.path().join("codex.child"))
            .expect("the script started its child before the limit")
            .trim()
            .parse()
            .expect("a pid");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while crate::settings_file_lock::process_alive(child)
            && std::time::Instant::now() < deadline
        {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            !crate::settings_file_lock::process_alive(child),
            "the question's child {child} outlived it"
        );
    }

    /// An answer kept in an older shape is asked again, never read
    /// (t-98207 X2). The mirror folder is named by the window's pid and
    /// nobody removes it, so a later window given that pid finds an older
    /// window's files under the same name. The shape before t-97731 kept
    /// subcommands alone: read now, `-C app` was the subcommand `app` and
    /// the TUI started without its words.
    #[cfg(unix)]
    #[test]
    fn an_answer_kept_in_an_older_shape_is_asked_again() {
        let dir = tempfile::tempdir().expect("a mirror directory and a fake codex");
        let mirrors = dir.path().join("mirrors");
        std::fs::create_dir(&mirrors).expect("mirrors");
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).expect("bin");
        let codex = fake_codex(&bin, "exit 0");
        let held = std::fs::metadata(&codex).expect("the fake's own facts");
        let modified = held
            .modified()
            .expect("a modification time")
            .duration_since(std::time::UNIX_EPOCH)
            .expect("after 1970")
            .as_nanos();
        let older_key = format!(
            "{}\t{modified}\t{}\t--no-daemon",
            codex.to_string_lossy(),
            held.len()
        );
        std::fs::write(
            mirrors.join(format!(
                "terminal-args-{}",
                crate::jev::fingerprint_of(&older_key)
            )),
            format!("{older_key}\nknows\nexec\napp\n"),
        )
        .expect("an older window's file");
        assert_eq!(
            with_terminal_args(
                &codex,
                os_words(&["-C", "app"]),
                ANSWERING_LIMIT,
                Some(&mirrors)
            ),
            (
                os_words(&["--no-daemon", "-C", "app"]),
                TerminalArgsOutcome::Added
            )
        );
        assert_eq!(
            times_asked(&codex),
            1,
            "the older file was read as an answer"
        );
        // What is kept now says its shape first.
        let kept = std::fs::read_dir(&mirrors)
            .expect("the mirror directory")
            .filter_map(|entry| std::fs::read_to_string(entry.ok()?.path()).ok())
            .filter(|kept| kept.starts_with(&format!("{KEPT_ANSWER_FORMAT}\t")))
            .count();
        assert_eq!(kept, 1);
    }

    /// What is kept reads back as it was said, an option whose value may be
    /// left out and one that takes many among it, and only under its own
    /// key.
    #[test]
    fn a_kept_answer_reads_back_as_it_was_said() {
        let dir = tempfile::tempdir().expect("a mirror directory");
        let file = dir.path().join("terminal-args-kept");
        let key = format!("{KEPT_ANSWER_FORMAT}\t/Users/dev/bin/codex\t1\t2\t--no-daemon");
        let option = |name: &str, many, optional| ValuedOption {
            name: name.to_string(),
            many,
            optional,
        };
        let help = HelpWords {
            commands: vec!["exec".to_string(), "e".to_string()],
            valued: vec![
                option("-C", false, false),
                option("-i", true, false),
                option("--foo", false, true),
                option("--bar", true, true),
            ],
        };
        for answer in [
            TerminalArgsAnswer::Knows(help),
            TerminalArgsAnswer::Refused,
            TerminalArgsAnswer::Unreadable,
        ] {
            keep_answer(&file, &key, &answer);
            assert_eq!(kept_answer(&file, &key), Some(answer));
            assert_eq!(kept_answer(&file, "another key"), None);
        }
    }

    /// On Windows the mirror is handed npm's `codex.cmd` (t-98207 X1): it is
    /// codex by the name it is called by, so it is asked and handed its
    /// words as `codex` is. A script by that name stands in for it here.
    #[cfg(unix)]
    #[test]
    fn a_codex_handed_over_as_its_cmd_file_is_still_codex() {
        let dir = tempfile::tempdir().expect("an npm folder");
        let cmd = dir.path().join("codex.cmd");
        std::fs::rename(fake_codex(dir.path(), "exit 0"), &cmd).expect("named as npm names it");
        assert_eq!(
            with_terminal_args(&cmd, os_words(&["fix it"]), ANSWERING_LIMIT, None),
            (
                os_words(&["--no-daemon", "fix it"]),
                TerminalArgsOutcome::Added
            )
        );
        assert_eq!(times_asked(&cmd), 1);
    }

    /// A fake binary in a directory of its own, for [`with_terminal_args`].
    #[cfg(unix)]
    fn fake_binary(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt as _;
        let held = dir.join(name);
        std::fs::write(&held, format!("#!/bin/sh\n{body}\n")).expect("a fake binary");
        std::fs::set_permissions(&held, std::fs::Permissions::from_mode(0o755))
            .expect("the executable bit");
        held
    }

    /// A fake codex >= 0.156.0: `--no-daemon --help` prints codex-cli
    /// 0.167.0's help and ends 0, after one line in `<itself>.asked`; any
    /// other start runs `rest`.
    #[cfg(unix)]
    fn fake_codex(dir: &std::path::Path, rest: &str) -> std::path::PathBuf {
        fake_binary(
            dir,
            "codex",
            &format!(
                "if [ \"$1\" = --no-daemon ] && [ \"$2\" = --help ] && [ $# -eq 2 ]; then\n\
                 echo asked >> \"$0.asked\"\n\
                 cat <<'HELP'\n{CODEX_0_167_HELP}HELP\n\
                 exit 0\n\
                 fi\n\
                 {rest}"
            ),
        )
    }

    /// How many times the fake at `binary` was asked about its words.
    #[cfg(unix)]
    fn times_asked(binary: &std::path::Path) -> usize {
        let mut asked = binary.as_os_str().to_owned();
        asked.push(".asked");
        std::fs::read_to_string(asked).map_or(0, |said| said.lines().count())
    }

    #[cfg(unix)]
    fn os_words(words: &[&str]) -> Vec<std::ffi::OsString> {
        words.iter().map(std::ffi::OsString::from).collect()
    }

    #[cfg(unix)]
    fn missed(binary: &std::path::Path, missed: TerminalArgsMissed) -> TerminalArgsOutcome {
        TerminalArgsOutcome::Missed(TerminalArgsMissedNote {
            agent: "codex".to_string(),
            binary: binary.to_string_lossy().into_owned(),
            missed,
        })
    }

    /// The words go in front only when the binary in hand says it knows
    /// them; an older codex, one whose help lists nothing, a silent one and a
    /// binary this table does not name all start exactly as they were asked,
    /// and the first three say why.
    #[cfg(unix)]
    #[test]
    fn the_words_go_in_front_only_when_the_binary_knows_them() {
        let dir = tempfile::tempdir().expect("a directory for fake binaries");
        // The real limit for every binary that answers: the first start of a
        // freshly written file is checked by the system first (130 ms by
        // hand, more under a parallel test run), and a shorter one here
        // failed the answering case on the build line.
        let limit = ANSWERING_LIMIT;
        // codex >= 0.156.0: `--no-daemon --help` answers 0 with its list.
        let knows = dir.path().join("knows");
        std::fs::create_dir(&knows).expect("dir");
        let codex = fake_codex(&knows, "exit 2");
        assert_eq!(
            with_terminal_args(&codex, os_words(&["resume", "019e7d20"]), limit, None),
            (
                os_words(&["--no-daemon", "resume", "019e7d20"]),
                TerminalArgsOutcome::Added
            )
        );
        // The window's planned launch keeps every word, in its order.
        let planned = launch_plan("codex", None).args;
        let planned: Vec<&str> = planned.iter().map(String::as_str).collect();
        let mut expected = vec!["--no-daemon"];
        expected.extend(&planned);
        assert_eq!(
            with_terminal_args(&codex, os_words(&planned), limit, None),
            (os_words(&expected), TerminalArgsOutcome::Added)
        );
        // A subcommand other than the interactive ones is left alone.
        for other in [&["agents"][..], &["exec", "summarize"], &["login"]] {
            assert_eq!(
                with_terminal_args(&codex, os_words(other), limit, None),
                (os_words(other), TerminalArgsOutcome::NotNeeded)
            );
        }

        // An option's value is read with the help the binary gave.
        let codex = knows.join("codex");
        assert_eq!(
            with_terminal_args(&codex, os_words(&["-C", "app"]), limit, None),
            (
                os_words(&["--no-daemon", "-C", "app"]),
                TerminalArgsOutcome::Added
            )
        );

        // codex older than 0.156.0 refuses the word (exit 2): it has no
        // shared daemon and needs no word, but a codex that dropped the flag
        // and kept the daemon refuses the same way, so the start that asked
        // says so (t-98207 X3).
        let old = dir.path().join("old");
        std::fs::create_dir(&old).expect("dir");
        let codex = fake_binary(&old, "codex", "exit 2");
        assert_eq!(
            with_terminal_args(&codex, os_words(&[]), limit, None),
            (Vec::new(), missed(&codex, TerminalArgsMissed::Refused))
        );

        // Any other failure is not a refusal, and is said as one.
        let broken = dir.path().join("broken");
        std::fs::create_dir(&broken).expect("dir");
        let codex = fake_binary(&broken, "codex", "exit 1");
        assert_eq!(
            with_terminal_args(&codex, os_words(&["fix it"]), limit, None),
            (
                os_words(&["fix it"]),
                missed(&codex, TerminalArgsMissed::Failed)
            )
        );

        // A binary that cannot even be started — Windows' extensionless npm
        // sh, here a file without its executable bit — starts as typed and
        // nothing is said: there was no answer to report.
        let unstartable = dir.path().join("unstartable");
        std::fs::create_dir(&unstartable).expect("dir");
        let codex = unstartable.join("codex");
        std::fs::write(&codex, "#!/bin/sh\nexit 0\n").expect("a file");
        assert_eq!(
            with_terminal_args(&codex, os_words(&["fix it"]), limit, None),
            (os_words(&["fix it"]), TerminalArgsOutcome::NotAsked)
        );

        // A codex that takes the word but lists no subcommand: a prompt
        // cannot be told from one.
        let listless = dir.path().join("listless");
        std::fs::create_dir(&listless).expect("dir");
        let codex = fake_binary(&listless, "codex", "echo 'Usage: codex'");
        assert_eq!(
            with_terminal_args(&codex, os_words(&["fix it"]), limit, None),
            (
                os_words(&["fix it"]),
                missed(&codex, TerminalArgsMissed::Unreadable)
            )
        );

        // A binary that does not answer is ended at the limit and the run
        // starts as typed — long before the binary would have answered.
        let silent = dir.path().join("silent");
        std::fs::create_dir(&silent).expect("dir");
        let codex = fake_binary(&silent, "codex", "exec sleep 30");
        let began = std::time::Instant::now();
        assert_eq!(
            with_terminal_args(
                &codex,
                os_words(&["fix it"]),
                std::time::Duration::from_millis(300),
                None
            ),
            (
                os_words(&["fix it"]),
                missed(&codex, TerminalArgsMissed::NoAnswer)
            )
        );
        assert!(began.elapsed() < std::time::Duration::from_secs(10));

        // Not an agent of this table: never asked anything.
        let other = fake_binary(dir.path(), "claude", r#"touch "$0.asked""#);
        assert_eq!(
            with_terminal_args(&other, os_words(&[]), limit, None),
            (Vec::new(), TerminalArgsOutcome::NotNeeded)
        );
        assert!(!dir.path().join("claude.asked").exists());
    }

    /// One question per binary (t-97498): two starts of the same codex ask
    /// it once, a codex replaced in place is asked again, a refusal is kept
    /// like a yes, and no answer in time is never kept.
    #[cfg(unix)]
    #[test]
    fn a_binary_is_asked_once_until_it_changes() {
        let dir = tempfile::tempdir().expect("a mirror directory and fake binaries");
        let mirrors = dir.path().join("mirrors");
        std::fs::create_dir(&mirrors).expect("mirrors");
        let limit = ANSWERING_LIMIT;
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).expect("bin");
        let codex = fake_codex(&bin, "exit 0");

        for _ in 0..2 {
            assert_eq!(
                with_terminal_args(&codex, os_words(&[]), limit, Some(&mirrors)).1,
                TerminalArgsOutcome::Added
            );
        }
        assert_eq!(times_asked(&codex), 1, "two starts, one question");
        // What was kept still tells a subcommand from a prompt.
        assert_eq!(
            with_terminal_args(&codex, os_words(&["exec", "go"]), limit, Some(&mirrors)).1,
            TerminalArgsOutcome::NotNeeded
        );
        // And an option's value from a subcommand.
        assert_eq!(
            with_terminal_args(&codex, os_words(&["-p", "review"]), limit, Some(&mirrors)).1,
            TerminalArgsOutcome::Added
        );
        assert_eq!(times_asked(&codex), 1);
        // Without a mirror directory nothing is kept.
        let _ = with_terminal_args(&codex, os_words(&[]), limit, None);
        assert_eq!(times_asked(&codex), 2);

        // An update writes a new binary over the old one.
        let codex = fake_codex(&bin, "exit 0\n# updated");
        let _ = with_terminal_args(&codex, os_words(&[]), limit, Some(&mirrors));
        assert_eq!(times_asked(&codex), 3, "a changed binary is asked again");

        // A refusal (exit 2) is an answer, and kept — and said once in this
        // window, by the start that asked; every later start reads it and
        // says nothing (t-98207 X3).
        let old = dir.path().join("old");
        std::fs::create_dir(&old).expect("old");
        let refusing = fake_binary(&old, "codex", r#"echo asked >> "$0.asked"; exit 2"#);
        assert_eq!(
            with_terminal_args(&refusing, os_words(&[]), limit, Some(&mirrors)).1,
            missed(&refusing, TerminalArgsMissed::Refused)
        );
        for _ in 0..2 {
            assert_eq!(
                with_terminal_args(&refusing, os_words(&[]), limit, Some(&mirrors)).1,
                TerminalArgsOutcome::NotNeeded
            );
        }
        assert_eq!(times_asked(&refusing), 1);

        // Any other failure is not: an update half written is asked again.
        let half = dir.path().join("half");
        std::fs::create_dir(&half).expect("half");
        let failing = fake_binary(&half, "codex", r#"echo asked >> "$0.asked"; exit 1"#);
        for _ in 0..2 {
            assert_eq!(
                with_terminal_args(&failing, os_words(&[]), limit, Some(&mirrors)).1,
                missed(&failing, TerminalArgsMissed::Failed)
            );
        }
        assert_eq!(times_asked(&failing), 2);

        // Nor is a binary that could not be started.
        let unstartable = dir.path().join("unstartable");
        std::fs::create_dir(&unstartable).expect("unstartable");
        let stranded = unstartable.join("codex");
        std::fs::write(&stranded, "#!/bin/sh\nexit 0\n").expect("a file");
        assert_eq!(
            with_terminal_args(&stranded, os_words(&[]), limit, Some(&mirrors)).1,
            TerminalArgsOutcome::NotAsked
        );

        // No answer in time is not: the next start asks again. Given the
        // real limit, as every case that must reach its first line is: a
        // shorter one ended the first start of this fresh file before it
        // could say it was asked (on the build line).
        let silent = dir.path().join("silent");
        std::fs::create_dir(&silent).expect("silent");
        let quiet = fake_binary(
            &silent,
            "codex",
            r#"echo asked >> "$0.asked"; exec sleep 30"#,
        );
        for _ in 0..2 {
            assert_eq!(
                with_terminal_args(
                    &quiet,
                    os_words(&[]),
                    TERMINAL_ARGS_PROBE_LIMIT,
                    Some(&mirrors)
                )
                .1,
                missed(&quiet, TerminalArgsMissed::NoAnswer)
            );
        }
        assert_eq!(times_asked(&quiet), 2);
        let kept = std::fs::read_dir(&mirrors)
            .expect("the mirror directory")
            .filter_map(|entry| std::fs::read_to_string(entry.ok()?.path()).ok())
            .collect::<Vec<_>>();
        for gave_none in [&quiet, &failing, &stranded] {
            let path = gave_none.to_string_lossy();
            assert!(
                !kept.iter().any(|kept| kept.starts_with(path.as_ref())),
                "an answer was kept for {path}, which gave none"
            );
        }
    }

    /// The question is asked uncoloured whatever the person's shell
    /// exported, so clap's coloured `Commands:` heading can never read as a
    /// help without a list (t-97498 P3-A).
    #[test]
    fn the_question_is_asked_without_colour() {
        let question = terminal_args_question(
            std::path::Path::new("/Users/dev/bin/codex"),
            &["--no-daemon".to_string()],
        );
        let envs: Vec<_> = question.get_envs().collect();
        assert!(
            envs.contains(&(
                std::ffi::OsStr::new("NO_COLOR"),
                Some(std::ffi::OsStr::new("1"))
            )),
            "{envs:?}"
        );
        assert!(
            envs.contains(&(std::ffi::OsStr::new("CLICOLOR_FORCE"), None)),
            "{envs:?}"
        );
        assert_eq!(
            question.get_args().collect::<Vec<_>>(),
            ["--no-daemon", "--help"]
        );
    }

    /// A codex that colours its help unless it is told not to still answers
    /// with its list.
    #[cfg(unix)]
    #[test]
    fn a_help_coloured_by_the_shell_still_reads() {
        let dir = tempfile::tempdir().expect("a fake codex");
        let codex = fake_binary(
            dir.path(),
            "codex",
            &format!(
                "if [ \"$NO_COLOR\" = 1 ] && [ -z \"${{CLICOLOR_FORCE+x}}\" ]; then\n\
                 cat <<'HELP'\n{CODEX_0_167_HELP}HELP\n\
                 else\n\
                 printf '\\033[1m\\033[4mCommands:\\033[0m\\n  exec  Run\\n'\n\
                 fi"
            ),
        );
        assert_eq!(
            with_terminal_args(&codex, os_words(&["-p", "review"]), ANSWERING_LIMIT, None),
            (
                os_words(&["--no-daemon", "-p", "review"]),
                TerminalArgsOutcome::Added
            )
        );
    }

    /// The line the window's log says for a missed start: this table's own
    /// words around the binary's path, and nothing for a note about anyone
    /// else or a payload that is not one.
    #[test]
    fn a_missed_start_reads_as_one_line_in_the_windows_log() {
        let note = |missed| TerminalArgsMissedNote {
            agent: "codex".to_string(),
            binary: "/Users/dev/bin/codex".to_string(),
            missed,
        };
        let line = |missed| TerminalArgsMissedNote::line_of(&note(missed).payload());
        assert_eq!(
            line(TerminalArgsMissed::Failed).as_deref(),
            Some(
                "codex at /Users/dev/bin/codex started without `--no-daemon`: \
                 `--no-daemon --help` failed — the commands it runs may wear another \
                 pane's identity"
            )
        );
        assert!(line(TerminalArgsMissed::NoAnswer).is_some_and(|said| {
            said.contains("did not answer `--no-daemon --help` within 2000 ms")
        }));
        assert_eq!(
            line(TerminalArgsMissed::Refused).as_deref(),
            Some(
                "codex at /Users/dev/bin/codex started without `--no-daemon`: it refused \
                 `--no-daemon --help` with exit 2, as a build older than the shared daemon \
                 does, and this window says so once — if it has the daemon, the commands it \
                 runs may wear another pane's identity"
            )
        );
        assert!(
            line(TerminalArgsMissed::Unreadable)
                .is_some_and(|said| said.contains("its help lists no subcommand"))
        );
        let mut forged = note(TerminalArgsMissed::Failed);
        forged.binary = "/Users/dev/bin/co\ndex\u{1b}[2J".to_string();
        let said = TerminalArgsMissedNote::line_of(&forged.payload()).expect("a line");
        assert!(!said.chars().any(char::is_control), "{said:?}");
        let mut other = note(TerminalArgsMissed::Failed);
        other.agent = "claude".to_string();
        assert_eq!(TerminalArgsMissedNote::line_of(&other.payload()), None);
        assert_eq!(TerminalArgsMissedNote::line_of("not a note"), None);
    }

    /// Two panes, one Codex home, one after the other — the shape of the
    /// person's report. The fake behaves as codex-cli 0.167.0 was measured to:
    /// without `--no-daemon` the first run leaves a daemon behind that keeps
    /// ITS environment, and a later run's command reads that one; with the
    /// flag each run's command reads its own. The real binary's version of
    /// this is the sandboxed live check in the hand-in.
    #[cfg(unix)]
    #[test]
    fn two_panes_running_codex_keep_their_own_identity() {
        let dir = tempfile::tempdir().expect("a Codex home and a fake codex");
        let home = dir.path().join("codex-home");
        std::fs::create_dir(&home).expect("home");
        let bin = dir.path().join("bin");
        std::fs::create_dir(&bin).expect("bin");
        let codex = fake_codex(
            &bin,
            r#"daemon="$CODEX_HOME/daemon-env"
if [ "$1" = --no-daemon ]; then
  seen="$ZEROCODE_PANE_KEY"
else
  [ -f "$daemon" ] || printf '%s' "$ZEROCODE_PANE_KEY" > "$daemon"
  seen=$(cat "$daemon")
fi
printf '%s\n' "$seen" >> "$CODEX_HOME/commands""#,
        );
        let run = |pane: &str, argv: Vec<std::ffi::OsString>| {
            let status = std::process::Command::new(&codex)
                .args(argv)
                .env("CODEX_HOME", &home)
                .env("ZEROCODE_PANE_KEY", pane)
                .status()
                .expect("the fake ran");
            assert!(status.success());
        };
        let seen = || std::fs::read_to_string(home.join("commands")).expect("commands");
        let limit = ANSWERING_LIMIT;

        // Before: started as typed, the second pane's command is the first's.
        run("term-1", os_words(&[]));
        run("term-2", os_words(&[]));
        assert_eq!(seen(), "term-1\nterm-1\n");

        // After: each start goes through the words, and each pane is its own.
        std::fs::remove_file(home.join("commands")).expect("reset");
        for pane in ["term-3", "term-4"] {
            run(
                pane,
                with_terminal_args(&codex, os_words(&[]), limit, None).0,
            );
        }
        assert_eq!(seen(), "term-3\nterm-4\n");
    }

    #[test]
    fn the_default_is_the_measured_flag_for_that_agent() {
        // Spot-checked across the shapes: a long flag, a short one, a flag
        // with a value, and one with a quoted value.
        assert_eq!(
            launch_plan("claude", None).args,
            ["--dangerously-skip-permissions"]
        );
        assert_eq!(
            launch_plan("grok", None).args,
            ["--permission-mode", "bypassPermissions"]
        );
        // The quoted default reaches the process the way the quotes meant:
        // one bare `*`, not a two-byte `"*"` the CLI would read as a literal
        // pattern that matches nothing (P0-9).
        assert_eq!(launch_plan("continue", None).args, ["--allow", "*"]);
        // An agent with no switch launches bare rather than being given
        // somebody else's flag.
        assert_eq!(launch_plan("droid", None).args, Vec::<String>::new());
        assert!(!has_permission_switch("droid"));
    }

    #[test]
    fn codex_starts_without_the_shared_daemon_the_managed_home_puts_out_of_reach() {
        // Measured on codex-cli 0.157.1: under the window's managed home the
        // shared daemon's socket path is 122 bytes where macOS's `sun_path`
        // holds 104, the connect is refused, and the tab ended with code 1 ten
        // seconds in. The compatibility words ride every codex launch —
        // default, edited or emptied — and are never the permission
        // judgement's or the settings field's business.
        let compat = ["-c", "features.daemon_auto_start=false"];
        let plain = launch_plan("codex", None);
        assert_eq!(
            plain.args,
            [
                "--dangerously-bypass-approvals-and-sandbox",
                compat[0],
                compat[1]
            ]
        );
        assert_eq!(plain.permission, PermissionMode::Unattended);

        let edited = LaunchOverride {
            args: Some("--model gpt-5.6".into()),
            env: None,
        };
        let plan = launch_plan("codex", Some(&edited));
        assert_eq!(plan.args, ["--model", "gpt-5.6", compat[0], compat[1]]);
        assert_eq!(plan.permission, PermissionMode::Mixed);

        let cleared = LaunchOverride {
            args: Some(String::new()),
            env: None,
        };
        let plan = launch_plan("codex", Some(&cleared));
        assert_eq!(plan.args, compat);
        assert_eq!(plan.permission, PermissionMode::Asks);

        // The field the settings pane shows and saves is the person's line,
        // not the window's compatibility words.
        assert!(!launch_args_line("codex", None).contains("daemon"));
        // Nobody else is handed codex's words.
        assert!(
            !launch_plan("claude", None)
                .args
                .iter()
                .any(|word| word.contains("daemon"))
        );
    }

    #[test]
    fn goose_is_told_by_the_environment_because_it_has_no_flag() {
        let plan = launch_plan("goose", None);
        assert!(plan.args.is_empty());
        assert_eq!(plan.env, [("GOOSE_MODE".to_string(), "auto".to_string())]);
        assert_eq!(plan.permission, PermissionMode::Unattended);
        assert!(has_permission_switch("goose"));
    }

    #[test]
    fn the_two_agents_that_refuse_the_flag_never_receive_it() {
        // Someone pastes claude's flag into opencode's field, which is exactly
        // how this would arrive in the wild.
        let held = LaunchOverride {
            args: Some("--dangerously-skip-permissions --verbose".into()),
            env: None,
        };
        assert_eq!(launch_plan("opencode", Some(&held)).args, ["--verbose"]);
        assert_eq!(launch_plan("kilo", Some(&held)).args, ["--verbose"]);
        // And a flag that merely starts the same way is not somebody's typo to
        // fix — it belongs to another tool.
        assert_eq!(
            sanitize_launch_args("opencode", "--dangerously-skip-permissions-maybe"),
            "--dangerously-skip-permissions-maybe"
        );
    }

    #[test]
    fn the_env_line_round_trips_through_the_field_it_is_shown_in() {
        // A value with a space comes back quoted, so the line the field shows
        // parses into the pairs it was rendered from.
        let held = LaunchOverride {
            args: None,
            env: Some(vec![
                ("A".to_string(), "plain".to_string()),
                ("B".to_string(), "with space".to_string()),
                ("C".to_string(), String::new()),
            ]),
        };
        let line = launch_env_line("claude", Some(&held));
        assert_eq!(line, r#"A=plain B="with space" C="""#);
        assert_eq!(
            split_command_line(&line).expect("the rendered line splits"),
            ["A=plain", "B=with space", "C="]
        );

        // Absent shows the measured default; present-and-empty shows empty.
        assert_eq!(launch_env_line("goose", None), "GOOSE_MODE=auto");
        let emptied = LaunchOverride {
            args: None,
            env: Some(Vec::new()),
        };
        assert_eq!(launch_env_line("goose", Some(&emptied)), "");
    }

    #[test]
    fn filtering_a_refused_flag_leaves_a_quoted_value_in_one_piece() {
        // The cross-harness bridge puts quoted `-c key="value"` in this field.
        // The old whitespace filter cut a value with a space into three
        // arguments the moment a refused flag was also present.
        let line = r#"--dangerously-skip-permissions -c base_url="http://127.0.0.1:8317/v1""#;
        let cleaned = sanitize_launch_args("opencode", line);
        assert_eq!(
            split_command_line(&cleaned).expect("clean line"),
            ["-c", r#"base_url=http://127.0.0.1:8317/v1"#],
            "filtering a refused flag collapsed a quoted value: {cleaned}"
        );

        // A quoted value that really does hold a space survives the round trip.
        let spaced = r#"--dangerously-skip-permissions --add-dir "with space""#;
        let kept = sanitize_launch_args("kilo", spaced);
        assert_eq!(
            split_command_line(&kept).expect("clean line"),
            ["--add-dir", "with space"],
            "a quoted path with a space did not survive filtering: {kept}"
        );

        // An agent that refuses nothing gets its line back byte for byte,
        // quotes intact — the common case, and every bridge target.
        let bridge = r#"-c model_providers.b.base_url="http://127.0.0.1:8317/v1""#;
        assert_eq!(sanitize_launch_args("codex", bridge), bridge);
    }

    #[test]
    fn an_emptied_field_means_ask_me_rather_than_use_the_default() {
        // The distinction the `Option` exists for: absent is "no opinion" and
        // present-but-empty is an opinion.
        let cleared = LaunchOverride {
            args: Some(String::new()),
            env: None,
        };
        let plan = launch_plan("claude", Some(&cleared));
        assert!(
            plan.args.is_empty(),
            "an emptied field got the default back"
        );
        assert_eq!(plan.permission, PermissionMode::Asks);

        assert_eq!(
            launch_plan("claude", None).permission,
            PermissionMode::Unattended
        );
    }

    #[test]
    fn an_edited_flag_is_neither_of_the_two_confident_words() {
        let edited = LaunchOverride {
            args: Some("--permission-mode plan".into()),
            env: None,
        };
        assert_eq!(
            launch_plan("claude", Some(&edited)).permission,
            PermissionMode::Mixed
        );
    }

    #[test]
    fn an_agent_told_by_half_is_mixed_rather_than_either() {
        // goose has only an env switch, so this is built by hand to exercise
        // the combine: one side told, the other not.
        assert_eq!(
            PermissionMode::combine(&[PermissionMode::Unattended, PermissionMode::Asks]),
            PermissionMode::Mixed
        );
        assert_eq!(
            PermissionMode::combine(&[PermissionMode::Unattended, PermissionMode::Unattended]),
            PermissionMode::Unattended
        );
        assert_eq!(
            PermissionMode::combine(&[PermissionMode::Asks, PermissionMode::Asks]),
            PermissionMode::Asks
        );
        assert_eq!(
            PermissionMode::combine(&[PermissionMode::Mixed, PermissionMode::Unattended]),
            PermissionMode::Mixed
        );
        // An agent with no switch at all: nothing to combine, and "asks" is
        // the truth — it will behave however it behaves.
        assert_eq!(PermissionMode::combine(&[]), PermissionMode::Asks);
    }

    #[test]
    fn an_env_override_replaces_rather_than_merges() {
        // A person who names one variable means that one, not that one plus
        // whatever we had in mind.
        let held = LaunchOverride {
            args: None,
            env: Some(vec![("GOOSE_MODE".into(), "approve".into())]),
        };
        let plan = launch_plan("goose", Some(&held));
        assert_eq!(
            plan.env,
            [("GOOSE_MODE".to_string(), "approve".to_string())]
        );
        assert_eq!(plan.permission, PermissionMode::Mixed);
    }

    #[test]
    fn the_global_permission_choice_changes_only_default_or_manual_fields() {
        let manual = apply_agent_permission_mode("claude", None, AgentPermissionMode::Manual)
            .expect("manual is an explicit override");
        assert_eq!(manual.args.as_deref(), Some(""));
        assert_eq!(
            launch_plan("claude", Some(&manual)).permission,
            PermissionMode::Asks
        );
        assert_eq!(
            apply_agent_permission_mode("claude", Some(&manual), AgentPermissionMode::Yolo),
            None,
            "Yolo returns an exact permission field to its measured default"
        );

        let custom = LaunchOverride {
            args: Some("--permission-mode plan".into()),
            env: Some(vec![("CLAUDE_CONFIG_DIR".into(), "/tmp/profile".into())]),
        };
        assert_eq!(
            apply_agent_permission_mode("claude", Some(&custom), AgentPermissionMode::Manual),
            Some(custom),
            "a global choice must not erase custom launch state"
        );
    }

    #[test]
    fn the_global_permission_choice_handles_environment_agents_without_args() {
        let unrelated = vec![("PROFILE".to_string(), "work".to_string())];
        let manual = apply_agent_permission_mode(
            "goose",
            Some(&LaunchOverride {
                args: Some("--verbose".into()),
                env: None,
            }),
            AgentPermissionMode::Manual,
        )
        .expect("the unrelated args remain alongside the manual env");
        assert_eq!(manual.args.as_deref(), Some("--verbose"));
        assert_eq!(manual.env, Some(Vec::new()));

        let custom_env = LaunchOverride {
            args: None,
            env: Some(unrelated),
        };
        assert_eq!(
            apply_agent_permission_mode("goose", Some(&custom_env), AgentPermissionMode::Yolo),
            Some(custom_env),
            "an environment override is not rewritten just because Yolo was chosen"
        );
    }

    #[test]
    fn rendered_argv_round_trips_hostile_worker_briefings_as_one_argument() {
        let prompt = concat!(
            "You are a worker in this window's orchestration.\n",
            "한국어와 JSON {\"ok\":true}, quotes \"stay\", and \\\\ paths survive."
        );
        let words = vec![
            "claude".to_string(),
            "--dangerously-skip-permissions".to_string(),
            prompt.to_string(),
            String::new(),
        ];
        let rendered = join_command_line(&words);
        assert_eq!(
            split_command_line(&rendered).expect("the rendered argv parses"),
            words,
            "the worker briefing changed at the command-line boundary: {rendered}"
        );
    }

    /// The splitter against Orca's own tokenizer rules
    /// (`tokenizeCustomCommandTemplate`) — quotes group and vanish, escapes
    /// take the next byte, an unclosed quote refuses.
    #[test]
    fn the_command_line_splits_the_way_its_quotes_meant() {
        let split = |line: &str| split_command_line(line).expect(line);
        assert_eq!(split("--allow \"*\""), ["--allow", "*"]);
        assert_eq!(
            split("--model sonnet --add-dir \"path with spaces\""),
            ["--model", "sonnet", "--add-dir", "path with spaces"]
        );
        // Leaving a quoted region keeps the word open, and quote styles mix.
        assert_eq!(split("a\"b c\"d"), ["ab cd"]);
        assert_eq!(split("--name 'Bob\"s'"), ["--name", "Bob\"s"]);
        // A backslash takes the next character, quoted or bare — including a
        // space, which is how a path escapes without quotes.
        assert_eq!(split(r"path\ with\ spaces"), ["path with spaces"]);
        assert_eq!(split(r#""a\"b""#), [r#"a"b"#]);
        // Empty quotes are a real, empty argument.
        assert_eq!(split("--flag ''"), ["--flag", ""]);
        assert_eq!(split("   "), Vec::<String>::new());
        // Unclosed quotes refuse rather than guess — Orca's own sentence.
        assert_eq!(
            split_command_line("--add-dir \"broken"),
            Err("Unclosed quote in command template.".to_string())
        );

        // The plan still launches over a legacy broken override: the old
        // whitespace split, not a refusal from a road with no dialog on it.
        let legacy = LaunchOverride {
            args: Some("--add-dir \"broken".into()),
            env: None,
        };
        assert_eq!(
            launch_plan("claude", Some(&legacy)).args,
            ["--add-dir", "\"broken"]
        );

        // And the settings row reads the LINE, not the re-joined words —
        // re-joining would strip the quotes that made one argument of a path.
        let quoted = LaunchOverride {
            args: Some("--add-dir \"path with spaces\"".into()),
            env: None,
        };
        assert_eq!(
            launch_args_line("claude", Some(&quoted)),
            "--add-dir \"path with spaces\""
        );
        assert_eq!(launch_args_line("continue", None), "--allow \"*\"");
        assert_eq!(
            launch_plan("claude", Some(&quoted)).args,
            ["--add-dir", "path with spaces"]
        );
    }
}
