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

    LaunchPlan {
        // Split the way the strings were written to be read: `continue`'s own
        // measured default is `--allow "*"`, and whitespace alone handed the
        // agent a literal two-byte `"*"` — and cut a person's quoted path
        // into pieces (P0-9). An unclosed quote falls back to the old
        // whitespace split so a legacy override still launches; the save
        // door refuses to store a new one.
        args: split_command_line(&args_said)
            .unwrap_or_else(|_| args_said.split_whitespace().map(str::to_string).collect()),
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
