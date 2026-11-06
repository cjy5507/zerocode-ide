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
    // `queue` and `remote-control <sub>` refuse `--no-daemon`; `agents`,
    // `archive`, `unarchive` and `delete` would work on a private server
    // instead of the shared one; `remote-control` alone would stay in the
    // foreground. Every other subcommand ignores the root flag.
    server_commands: &[
        "agents",
        "queue",
        "archive",
        "unarchive",
        "delete",
        "remote-control",
    ],
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
    /// Subcommands whose job is the shared server itself. Such a run is left
    /// exactly as it was started.
    pub server_commands: &'static [&'static str],
}

/// How long [`with_terminal_args`] waits for an agent to say whether it
/// knows its words. codex-cli 0.166.0 answered in 60 ms and 0.167.0 in 90 to
/// 320 ms; an agent that has not answered by then starts as it was typed.
pub const TERMINAL_ARGS_PROBE_LIMIT: std::time::Duration = std::time::Duration::from_secs(2);

/// The words [`AGENT_TERMINAL_ARGS`] puts in front of one interactive run of
/// `agent` with these arguments — empty when the agent has none, when the run
/// already chose its server, when it is about the shared server itself, and
/// when it only asks the binary about itself. Everything after a bare `--`
/// is data, whatever it is spelled like.
#[must_use]
pub fn terminal_args<S: AsRef<str>>(agent: &str, argv: &[S]) -> Vec<String> {
    let Some(rule) = AGENT_TERMINAL_ARGS.iter().find(|rule| rule.agent == agent) else {
        return Vec::new();
    };
    let mut words = argv
        .iter()
        .map(|word| word.as_ref())
        .take_while(|word| *word != "--");
    let left_alone = words.any(|word| {
        rule.server_commands.contains(&word)
            || rule.chosen_by.iter().any(|flag| {
                word.strip_prefix(flag)
                    .is_some_and(|rest| rest.is_empty() || rest.starts_with('='))
            })
    });
    if left_alone || crate::hook::is_agent_probe_argv(argv) {
        return Vec::new();
    }
    rule.args.split_whitespace().map(str::to_string).collect()
}

/// The arguments an interactive run of `program` starts with: its own, with
/// the [`terminal_args`] of the agent it is in front — but only when this
/// binary knows them.
///
/// Whether it does is asked of the binary itself, `program <words> --version`
/// within `limit`: a CLI that parses its arguments before acting on
/// `--version` refuses a word it does not know (exit 2, measured on
/// codex-cli 0.166.0 and 0.167.0), and a codex older than 0.156.0 has neither
/// the flag nor the daemon it keeps away. No answer, a refusal or a binary
/// this table does not name: the run starts exactly as it was asked.
#[must_use]
pub fn with_terminal_args(
    program: &std::path::Path,
    argv: Vec<std::ffi::OsString>,
    limit: std::time::Duration,
) -> Vec<std::ffi::OsString> {
    let Some(agent) = program
        .file_stem()
        .and_then(std::ffi::OsStr::to_str)
        .and_then(|name| {
            crate::AGENT_SPECS
                .iter()
                .find(|spec| spec.detect_names().any(|said| said == name))
        })
        .map(|spec| spec.id)
    else {
        return argv;
    };
    let words = terminal_args(agent, &crate::hook::argv_text(&argv));
    if words.is_empty() || !knows_words_within(program, &words, limit) {
        return argv;
    }
    words
        .into_iter()
        .map(std::ffi::OsString::from)
        .chain(argv)
        .collect()
}

/// Whether `program <words> --version` ends with success within `limit`.
/// Ended rather than abandoned when it does not, so no probe outlives the
/// run it was asked for. Its one caller is the mirror, a console program in
/// the person's terminal, so the probe shares that console as the run does.
fn knows_words_within(
    program: &std::path::Path,
    words: &[String],
    limit: std::time::Duration,
) -> bool {
    use std::process::{Command, Stdio};
    let Ok(mut child) = Command::new(program)
        .args(words)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let began = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if began.elapsed() < limit => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
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

    /// The words every interactive `codex` in a ZeroCode terminal wears, and
    /// the runs that are left exactly as they were started (t-97015).
    ///
    /// Read from codex-cli 0.167.0's `cli/src/main.rs`: the root
    /// `--no-daemon` reaches the TUI, `resume` and `fork`; every subcommand
    /// but the six named below ignores it (measured: `codex --no-daemon exec
    /// --help` and `… login --help` both end 0).
    #[test]
    fn an_interactive_codex_keeps_its_commands_in_its_own_terminal() {
        let words = |argv: &[&str]| terminal_args("codex", argv);
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
        assert_eq!(words(&["exec", "summarize"]), kept);
        // The window's own launch wears them too — its plan is unchanged and
        // the words are added in front of it, not into it.
        assert_eq!(
            terminal_args("codex", &launch_plan("codex", None).args),
            kept
        );
        // A prompt that merely names a subcommand is one word, not that one.
        assert_eq!(words(&["explain codex agents"]), kept);
        // After a bare `--` everything is data.
        assert_eq!(words(&["--", "--remote"]), kept);

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
        // Nobody else is handed codex's words.
        assert!(terminal_args("claude", &[] as &[&str]).is_empty());
        assert!(terminal_args("zo", &[] as &[&str]).is_empty());
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

    #[cfg(unix)]
    fn os_words(words: &[&str]) -> Vec<std::ffi::OsString> {
        words.iter().map(std::ffi::OsString::from).collect()
    }

    /// The words go in front only when the binary in hand says it knows
    /// them; an older codex, a silent one and a binary this table does not
    /// name all start exactly as they were asked.
    #[cfg(unix)]
    #[test]
    fn the_words_go_in_front_only_when_the_binary_knows_them() {
        let dir = tempfile::tempdir().expect("a directory for fake binaries");
        // The real limit for every binary that answers: the first start of a
        // freshly written file is checked by the system first (130 ms by
        // hand, more under a parallel test run), and a shorter one here
        // failed the answering case on the build line.
        let limit = TERMINAL_ARGS_PROBE_LIMIT;
        // codex >= 0.156.0: `--no-daemon --version` answers 0.
        let knows = dir.path().join("knows");
        std::fs::create_dir(&knows).expect("dir");
        let codex = fake_binary(
            &knows,
            "codex",
            r#"[ "$1" = --no-daemon ] && [ "$2" = --version ] && [ $# -eq 2 ] && exit 0
exit 2"#,
        );
        assert_eq!(
            with_terminal_args(&codex, os_words(&["resume", "019e7d20"]), limit),
            os_words(&["--no-daemon", "resume", "019e7d20"])
        );
        // The window's planned launch keeps every word, in its order.
        let planned = launch_plan("codex", None).args;
        let planned: Vec<&str> = planned.iter().map(String::as_str).collect();
        let mut expected = vec!["--no-daemon"];
        expected.extend(&planned);
        assert_eq!(
            with_terminal_args(&codex, os_words(&planned), limit),
            os_words(&expected)
        );
        // A run left alone is not even asked about.
        assert_eq!(
            with_terminal_args(&codex, os_words(&["agents"]), limit),
            os_words(&["agents"])
        );

        // codex older than 0.156.0 refuses the word (exit 2).
        let old = dir.path().join("old");
        std::fs::create_dir(&old).expect("dir");
        let codex = fake_binary(&old, "codex", "exit 2");
        assert_eq!(
            with_terminal_args(&codex, os_words(&[]), limit),
            Vec::<std::ffi::OsString>::new()
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
                std::time::Duration::from_millis(300)
            ),
            os_words(&["fix it"])
        );
        assert!(began.elapsed() < std::time::Duration::from_secs(10));

        // Not an agent of this table: never asked anything.
        let other = fake_binary(dir.path(), "claude", r#"touch "$0.asked""#);
        assert_eq!(
            with_terminal_args(&other, os_words(&[]), limit),
            Vec::<std::ffi::OsString>::new()
        );
        assert!(!dir.path().join("claude.asked").exists());
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
        let codex = fake_binary(
            &bin,
            "codex",
            r#"[ "$1" = --no-daemon ] && [ "$2" = --version ] && exit 0
daemon="$CODEX_HOME/daemon-env"
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
        let limit = TERMINAL_ARGS_PROBE_LIMIT;

        // Before: started as typed, the second pane's command is the first's.
        run("term-1", os_words(&[]));
        run("term-2", os_words(&[]));
        assert_eq!(seen(), "term-1\nterm-1\n");

        // After: each start goes through the words, and each pane is its own.
        std::fs::remove_file(home.join("commands")).expect("reset");
        for pane in ["term-3", "term-4"] {
            run(pane, with_terminal_args(&codex, os_words(&[]), limit));
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
