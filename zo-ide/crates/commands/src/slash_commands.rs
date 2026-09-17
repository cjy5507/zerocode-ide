use std::fmt;
use std::time::Duration;

use crate::mcp_command::McpAction;
use crate::remote_command::{RemoteAction, parse_remote_action};
use crate::slash_help::render_slash_command_help_detail;

pub const MAX_SESSION_NAME_CHARS: usize = 24;
pub const DEEP_TIER_USAGE: &str =
    "Usage: /tier [add <model>|set <N|model> <model>|remove <model|N>|move <N> <M>|reset]";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelfImproveAction {
    Propose,
    Apply { patch_digest: String },
    Status,
    Show { proposal_id: String },
    Review { proposal_id: String },
    Reject { proposal_id: String },
}

/// Exact 64-hex SHA-256 form shared by `/improve` proposal IDs and patch
/// digests.
fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeepTierAction {
    Show,
    Add { model: String },
    /// Replace the entry at `target` (1-based position or model name) in place,
    /// keeping its rank. Swapping a planner without `remove` + `add` + `move`
    /// is the difference between one keystroke and three, and it is the only
    /// mutation that cannot change the pool's order by accident.
    Set { target: String, model: String },
    Remove { target: String },
    Move { from: usize, to: usize },
    Reset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceRewindAction {
    List,
    Restore {
        turn_index: usize,
        force: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashCommand {
    Help,
    Status,
    /// `/compact [instructions]` — compact the session. The optional remainder is
    /// a free-text focus hint (e.g. `/compact focus on auth`) passed through to
    /// the compactor so the user can steer what the summary preserves.
    Compact {
        instructions: Option<String>,
    },
    Bughunter {
        scope: Option<String>,
    },
    Commit,
    Ship {
        message: String,
    },
    Pr {
        context: Option<String>,
    },
    Issue {
        context: Option<String>,
    },
    Ultraplan {
        task: Option<String>,
    },
    Council {
        task: Option<String>,
    },
    Distill {
        topic: Option<String>,
    },
    /// Dreamer self-repair: read the top self-improvement candidate, generate a
    /// fix in an isolated worktree, validate it through the quarantine + manual
    /// apply gate, and (only with `apply` + explicit approval) apply it.
    SelfImprove {
        /// `/improve` (propose), `/improve status`, and the review-first
        /// lifecycle on an exact proposal ID: `show`, `review`, `reject`,
        /// `apply` (all take the 64-hex SHA-256 ID).
        action: SelfImproveAction,
    },
    /// Pair a phone over Tailscale with this live local session.
    Remote {
        action: RemoteAction,
    },
    /// Bounded closed-loop automation: act, validate, and repair until the goal
    /// succeeds or a stop condition is reached.
    Goal {
        command: GoalCommand,
    },
    /// Session-local recurring prompt scheduler.
    Loop {
        command: LoopCommand,
    },
    Teleport {
        target: Option<String>,
    },
    DebugToolCall,
    Model {
        model: Option<String>,
    },
    Permissions {
        mode: Option<String>,
    },
    Clear {
        confirm: bool,
    },
    Cost,
    Resume {
        session_path: Option<String>,
    },
    Config {
        section: Option<String>,
    },
    Mcp {
        action: Option<String>,
        target: Option<String>,
    },
    Tools,
    Memory,
    Dream,
    /// `/refine` — the evidence-backed tuning report: harness attest health,
    /// learned-specialty shadow deltas, and the improve-pipeline hand-off.
    /// Read-only by contract; every finding names the command that would
    /// apply it.
    Refine,
    Init,
    Diff,
    Version,
    Export {
        path: Option<String>,
    },
    /// `/dump` — write the raw transcript to a temp artifact and open it in
    /// `$PAGER` (default `less`, which gives real `/pattern` search); `/dump
    /// edit` opens the same artifact in `$EDITOR` instead. The escape hatch
    /// out of the alt-screen for reading or grepping long sessions.
    Dump {
        edit: bool,
    },
    Session {
        action: Option<String>,
        target: Option<String>,
    },
    Name {
        name: Option<String>,
    },
    Plugins {
        action: Option<String>,
        target: Option<String>,
    },
    Agents {
        args: Option<String>,
    },
    Inbox {
        args: Option<String>,
    },
    Skills {
        args: Option<String>,
    },
    Doctor,
    Login {
        provider: Option<String>,
    },
    Logout {
        /// `all` clears every saved OAuth credential at once; `None` opens the
        /// manager so one account can be disconnected deliberately.
        scope: Option<String>,
    },
    Upgrade,
    /// `/restart` — persist the session, restore the terminal, and re-exec the
    /// newest build on disk, resuming this session. Backs the stale-binary
    /// sidebar badge ("/restart · new build on disk …").
    Restart,
    /// `/audit` surfaces the tool-invocation ledger for this session
    /// (per-tool counts, permission denials, route decisions) — the operator
    /// view of the model-facing `Audit` tool.
    Audit,
    /// `/share` writes a local artifact; `/share gist` uploads it to a secret
    /// GitHub gist. `target` is `None` (local) or `Some("gist")` (hosted).
    Share {
        target: Option<String>,
    },
    /// `/unshare <id>` deletes a previously created share gist.
    Unshare {
        id: Option<String>,
    },
    Feedback,
    Files,
    Fast {
        mode: Option<String>,
    },
    /// `/smart` opens Smart Model Router GUI; `/smart status` shows current setup.
    Smart {
        arg: Option<String>,
    },
    DeepTier {
        action: DeepTierAction,
    },
    New,
    Connect {
        provider: Option<String>,
    },
    /// Register / edit / delete model providers in one place.
    Providers,
    Exit,
    Desktop,
    Brief,
    Advisor,
    Insights,
    Thinkback,
    ReleaseNotes,
    PrComments {
        pr_number: Option<String>,
    },
    CommitPushPr,
    BackfillSessions,
    ExtraUsage,
    PerfIssue,
    Statusline,
    AntTrace,
    SecurityReview,
    Keybindings,
    PrivacySettings,
    Plan {
        mode: Option<String>,
    },
    /// `/deep [check-cmd|off]` — toggle the deep-lane gate (plan → implement →
    /// verify → retry). `check` carries the objective green command, the literal
    /// `off`, or `None` for verifier-only.
    Deep {
        check: Option<String>,
    },
    /// `/auto [check-cmd|on|off]` — toggle reactive auto-verify (the interactive
    /// default): edits are auto-verified and retried in one shot. `arg` is the
    /// objective green command, `on`/`off`, or `None` to auto-detect.
    Auto {
        arg: Option<String>,
    },
    Review {
        scope: Option<String>,
    },
    Hunks,
    Tasks {
        args: Option<String>,
    },
    Cache,
    Fork {
        name: Option<String>,
    },
    Theme {
        name: Option<String>,
    },
    Usage {
        scope: Option<String>,
    },
    Rename {
        name: Option<String>,
    },
    Copy {
        target: Option<String>,
    },
    Hooks {
        args: Option<String>,
    },
    ReloadContext,
    Context {
        action: Option<String>,
    },
    Effort {
        level: Option<String>,
    },
    Branch {
        name: Option<String>,
    },
    Rewind {
        action: WorkspaceRewindAction,
    },
    Undo {
        steps: Option<String>,
    },
    Redo {
        steps: Option<String>,
    },
    Focus,
    Ide {
        target: Option<String>,
    },
    OutputStyle {
        style: Option<String>,
    },
    AddDir {
        path: Option<String>,
    },
    Unknown {
        name: String,
        args: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GoalCommand {
    /// Show the active goal and validator status.
    Status,
    /// Start a new bounded goal loop.
    Start { goal: String, options: GoalOptions },
    /// Run validators without asking the model to act first.
    Verify,
    /// Temporarily stop automatic follow-up turns.
    Pause,
    /// Resume a paused goal and enqueue/execute the next action turn.
    Resume,
    /// Cancel and forget the active goal.
    Clear,
    /// Show recent goal outcomes recorded in this session.
    History,
    /// Replace the goal text while keeping the current validator/limit config.
    Edit { goal: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct GoalOptions {
    pub checks: Vec<String>,
    pub max_turns: Option<u32>,
    pub token_budget: Option<u64>,
    /// `--allow-writes` opt-in: when set, the goal's unattended action/repair
    /// turns inherit the session's write permission instead of being forced
    /// read-only + propose-only. Defaults to `false` (the safe unattended
    /// default), so an omitted flag keeps a goal read-only.
    pub allow_writes: bool,
    pub no_plan: bool,
    pub until: Option<String>,
    pub every: Option<DurationSpec>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LoopCommand {
    /// List active session loops.
    List,
    /// Show all active loops or one specific loop id.
    Status { id: Option<String> },
    /// Start a fixed-count loop: `/loop 5 <prompt>`.
    StartFixedCount { count: u32, prompt: String },
    /// Start immediately and let `loop_schedule` choose later wakeups.
    StartDynamic { prompt: String },
    /// Start an interval loop: `/loop every 10m <prompt>`.
    StartInterval { every: DurationSpec, prompt: String },
    /// Start a polling file-watch loop: `/loop watch <glob> <prompt>`.
    StartWatch { glob: String, prompt: String },
    /// Poll a shell condition and run a final iteration when it succeeds.
    StartUntil { check: String, prompt: String },
    /// Run a loop once immediately.
    RunNow { id: Option<String> },
    /// Pause one loop.
    Pause { id: Option<String> },
    /// Resume one loop.
    Resume { id: Option<String> },
    /// Stop one loop, the most recent loop, or every loop.
    Stop { id: Option<String>, all: bool },
    /// Remove stopped loops from the in-memory list.
    Clear,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DurationSpec {
    pub raw: String,
    pub duration: Duration,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlashCommandParseError {
    message: String,
}

impl SlashCommandParseError {
    pub(crate) fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

impl fmt::Display for SlashCommandParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SlashCommandParseError {}

impl SlashCommand {
    pub fn parse(input: &str) -> Result<Option<Self>, SlashCommandParseError> {
        validate_slash_command_input(input)
    }
}

#[allow(clippy::too_many_lines)] // flat slash-command parse table, one thin arm per command
pub fn validate_slash_command_input(
    input: &str,
) -> Result<Option<SlashCommand>, SlashCommandParseError> {
    let trimmed = input.trim();
    if !trimmed.starts_with('/') {
        return Ok(None);
    }

    let mut parts = trimmed.trim_start_matches('/').split_whitespace();
    let command = parts.next().unwrap_or_default();
    if command.is_empty() {
        return Err(SlashCommandParseError::new(
            "Slash command name is missing. Use /help to list available slash commands.",
        ));
    }

    let args = parts.collect::<Vec<_>>();
    let remainder = remainder_after_command(trimmed, command);

    Ok(Some(match command {
        "help" => {
            validate_no_args(command, &args)?;
            SlashCommand::Help
        }
        // `/stats` and `/summary` were thin subsets of the `/status` card;
        // kept as aliases so muscle memory still lands somewhere useful.
        "status" | "stats" | "summary" => {
            validate_no_args(command, &args)?;
            SlashCommand::Status
        }
        // `/sandbox` only ever pointed at `/permissions`; alias it outright.
        "sandbox" => {
            validate_no_args(command, &args)?;
            SlashCommand::Permissions { mode: None }
        }
        "compact" => SlashCommand::Compact {
            instructions: remainder,
        },
        "bughunter" => SlashCommand::Bughunter { scope: remainder },
        "commit" => {
            validate_no_args(command, &args)?;
            SlashCommand::Commit
        }
        "ship" => SlashCommand::Ship {
            message: require_remainder(command, remainder, "<commit-message>")?,
        },
        "pr" => SlashCommand::Pr { context: remainder },
        "issue" => SlashCommand::Issue { context: remainder },
        "ultraplan" => SlashCommand::Ultraplan { task: remainder },
        "council" => SlashCommand::Council { task: remainder },
        "distill" => SlashCommand::Distill { topic: remainder },
        "improve" => SlashCommand::SelfImprove {
            action: match args.as_slice() {
                [] => SelfImproveAction::Propose,
                ["status"] => SelfImproveAction::Status,
                ["apply", patch_digest] if is_sha256_hex(patch_digest) => {
                    SelfImproveAction::Apply {
                        patch_digest: (*patch_digest).to_ascii_lowercase(),
                    }
                }
                ["show", proposal_id] if is_sha256_hex(proposal_id) => SelfImproveAction::Show {
                    proposal_id: (*proposal_id).to_ascii_lowercase(),
                },
                ["review", proposal_id] if is_sha256_hex(proposal_id) => {
                    SelfImproveAction::Review {
                        proposal_id: (*proposal_id).to_ascii_lowercase(),
                    }
                }
                ["reject", proposal_id] if is_sha256_hex(proposal_id) => {
                    SelfImproveAction::Reject {
                        proposal_id: (*proposal_id).to_ascii_lowercase(),
                    }
                }
                _ => {
                    return Err(SlashCommandParseError::new(
                        "Usage: /improve [status|show <id>|review <id>|reject <id>|apply <64-character SHA-256 proposal ID>]",
                    ));
                }
            },
        },
        "remote" => SlashCommand::Remote {
            action: parse_remote_action(&args)?,
        },
        "goal" => SlashCommand::Goal {
            command: parse_goal_command(remainder.as_deref())?,
        },
        "loop" => SlashCommand::Loop {
            command: parse_loop_command(remainder.as_deref())?,
        },
        "teleport" => SlashCommand::Teleport {
            target: Some(require_remainder(command, remainder, "<symbol-or-path>")?),
        },
        "debug-tool-call" => {
            validate_no_args(command, &args)?;
            SlashCommand::DebugToolCall
        }
        "model" => SlashCommand::Model {
            model: optional_single_arg(command, &args, "[model]")?,
        },
        "permissions" => SlashCommand::Permissions {
            mode: parse_permissions_mode(&args)?,
        },
        "clear" => SlashCommand::Clear {
            confirm: parse_clear_args(&args)?,
        },
        "cost" => {
            validate_no_args(command, &args)?;
            SlashCommand::Cost
        }
        "resume" => SlashCommand::Resume {
            session_path: remainder,
        },
        "config" => SlashCommand::Config {
            section: parse_config_section(&args)?,
        },
        "mcp" => parse_mcp_command(&args)?,
        "tools" => {
            validate_no_args(command, &args)?;
            SlashCommand::Tools
        }
        "memory" => {
            validate_no_args(command, &args)?;
            SlashCommand::Memory
        }
        "dream" => {
            validate_no_args(command, &args)?;
            SlashCommand::Dream
        }
        "refine" => {
            validate_no_args(command, &args)?;
            SlashCommand::Refine
        }
        "init" => {
            validate_no_args(command, &args)?;
            SlashCommand::Init
        }
        "diff" => {
            validate_no_args(command, &args)?;
            SlashCommand::Diff
        }
        "version" => {
            validate_no_args(command, &args)?;
            SlashCommand::Version
        }
        "export" => SlashCommand::Export { path: remainder },
        "dump" => match args.as_slice() {
            [] => SlashCommand::Dump { edit: false },
            ["edit"] => SlashCommand::Dump { edit: true },
            _ => {
                return Err(SlashCommandParseError::new(
                    "Usage: /dump [edit] — open the transcript dump in $PAGER, or $EDITOR with `edit`.",
                ));
            }
        },
        "session" => parse_session_command(&args)?,
        "name" => parse_name_command(remainder)?,
        "plugin" | "plugins" | "marketplace" => parse_plugin_command(&args)?,
        "agents" => SlashCommand::Agents {
            args: parse_list_or_help_args(command, remainder)?,
        },
        "inbox" | "teaminbox" => SlashCommand::Inbox {
            args: remainder,
        },
        "skills" => SlashCommand::Skills {
            args: parse_skills_args(remainder.as_deref())?,
        },
        "doctor" => {
            validate_no_args(command, &args)?;
            SlashCommand::Doctor
        }
        "login" => SlashCommand::Login {
            provider: args.first().map(std::string::ToString::to_string),
        },
        "logout" => SlashCommand::Logout {
            scope: args.first().map(std::string::ToString::to_string),
        },
        "upgrade" => {
            validate_no_args(command, &args)?;
            SlashCommand::Upgrade
        }
        "restart" => {
            validate_no_args(command, &args)?;
            SlashCommand::Restart
        }
        "audit" => {
            validate_no_args(command, &args)?;
            SlashCommand::Audit
        }
        "share" => SlashCommand::Share {
            target: parse_share_target(command, &args)?,
        },
        "unshare" => SlashCommand::Unshare { id: remainder },
        "feedback" => {
            validate_no_args(command, &args)?;
            SlashCommand::Feedback
        }
        "files" => {
            validate_no_args(command, &args)?;
            SlashCommand::Files
        }
        "fast" => SlashCommand::Fast { mode: remainder },
        "smart" => SlashCommand::Smart { arg: remainder },
        "tier" => SlashCommand::DeepTier {
            action: match args.as_slice() {
                [] => DeepTierAction::Show,
                ["add", model] => DeepTierAction::Add {
                    model: (*model).to_string(),
                },
                ["set", target, model] => DeepTierAction::Set {
                    target: (*target).to_string(),
                    model: (*model).to_string(),
                },
                ["remove", target] => DeepTierAction::Remove {
                    target: (*target).to_string(),
                },
                ["move", from, to] => DeepTierAction::Move {
                    from: parse_deep_tier_index(from)?,
                    to: parse_deep_tier_index(to)?,
                },
                ["reset"] => DeepTierAction::Reset,
                _ => {
                    return Err(SlashCommandParseError::new(DEEP_TIER_USAGE));
                }
            },
        },
        "new" => {
            validate_no_args(command, &args)?;
            SlashCommand::New
        }
        "connect" => SlashCommand::Connect {
            provider: args.first().map(std::string::ToString::to_string),
        },
        "providers" => {
            validate_no_args(command, &args)?;
            SlashCommand::Providers
        }
        "exit" => {
            validate_no_args(command, &args)?;
            SlashCommand::Exit
        }
        "desktop" => {
            validate_no_args(command, &args)?;
            SlashCommand::Desktop
        }
        "brief" => {
            validate_no_args(command, &args)?;
            SlashCommand::Brief
        }
        "advisor" => {
            validate_no_args(command, &args)?;
            SlashCommand::Advisor
        }
        "insights" => {
            validate_no_args(command, &args)?;
            SlashCommand::Insights
        }
        "thinkback" => {
            validate_no_args(command, &args)?;
            SlashCommand::Thinkback
        }
        "release-notes" => {
            validate_no_args(command, &args)?;
            SlashCommand::ReleaseNotes
        }
        "pr-comments" | "pr_comments" => SlashCommand::PrComments {
            pr_number: remainder,
        },
        "commit-push-pr" => {
            validate_no_args(command, &args)?;
            SlashCommand::CommitPushPr
        }
        "backfill-sessions" => {
            validate_no_args(command, &args)?;
            SlashCommand::BackfillSessions
        }
        "extra-usage" => {
            validate_no_args(command, &args)?;
            SlashCommand::ExtraUsage
        }
        "perf-issue" => {
            validate_no_args(command, &args)?;
            SlashCommand::PerfIssue
        }
        "statusline" => {
            validate_no_args(command, &args)?;
            SlashCommand::Statusline
        }
        "ant-trace" => {
            validate_no_args(command, &args)?;
            SlashCommand::AntTrace
        }
        "security-review" => {
            validate_no_args(command, &args)?;
            SlashCommand::SecurityReview
        }
        "keybindings" => {
            validate_no_args(command, &args)?;
            SlashCommand::Keybindings
        }
        "privacy-settings" => {
            validate_no_args(command, &args)?;
            SlashCommand::PrivacySettings
        }
        "plan" => SlashCommand::Plan { mode: remainder },
        "deep" => SlashCommand::Deep { check: remainder },
        "auto" => SlashCommand::Auto { arg: remainder },
        "review" => SlashCommand::Review { scope: remainder },
        "hunks" => {
            validate_no_args(command, &args)?;
            SlashCommand::Hunks
        }
        // `/todos` is the Claude Code name for the same surface.
        "tasks" | "todos" => SlashCommand::Tasks { args: remainder },
        "cache" => SlashCommand::Cache,
        "fork" => SlashCommand::Fork { name: remainder },
        "theme" => SlashCommand::Theme { name: remainder },
        "usage" => {
            validate_no_args(command, &args)?;
            SlashCommand::Usage { scope: None }
        }
        "rename" => SlashCommand::Rename { name: remainder },
        "copy" => SlashCommand::Copy { target: remainder },
        "hooks" => SlashCommand::Hooks { args: remainder },
        "reload-context" => {
            validate_no_args(command, &args)?;
            SlashCommand::ReloadContext
        }
        "context" => SlashCommand::Context { action: remainder },
        "effort" => SlashCommand::Effort { level: remainder },
        "branch" => SlashCommand::Branch { name: remainder },
        "rewind" => SlashCommand::Rewind {
            action: match args.as_slice() {
                [] => WorkspaceRewindAction::List,
                [turn] => WorkspaceRewindAction::Restore {
                    turn_index: parse_rewind_turn(turn)?,
                    force: false,
                },
                [turn, "force"] => WorkspaceRewindAction::Restore {
                    turn_index: parse_rewind_turn(turn)?,
                    force: true,
                },
                _ => return Err(rewind_usage_error()),
            },
        },
        "undo" => SlashCommand::Undo { steps: remainder },
        "redo" => SlashCommand::Redo { steps: remainder },
        "focus" => SlashCommand::Focus,
        "ide" => SlashCommand::Ide { target: remainder },
        "output-style" => SlashCommand::OutputStyle { style: remainder },
        "add-dir" => SlashCommand::AddDir { path: remainder },
        other => SlashCommand::Unknown {
            name: other.to_string(),
            args: remainder,
        },
    }))
}

fn parse_deep_tier_index(value: &str) -> Result<usize, SlashCommandParseError> {
    value
        .parse::<usize>()
        .ok()
        .filter(|index| *index > 0)
        .ok_or_else(|| SlashCommandParseError::new(DEEP_TIER_USAGE))
}
fn validate_no_args(command: &str, args: &[&str]) -> Result<(), SlashCommandParseError> {
    if args.is_empty() {
        return Ok(());
    }

    Err(command_error(
        &format!("Unexpected arguments for /{command}."),
        command,
        &format!("/{command}"),
    ))
}

fn parse_rewind_turn(turn: &str) -> Result<usize, SlashCommandParseError> {
    turn.parse::<usize>()
        .ok()
        .filter(|turn| *turn > 0)
        .ok_or_else(rewind_usage_error)
}

fn rewind_usage_error() -> SlashCommandParseError {
    SlashCommandParseError::new("Usage: /rewind [<turn> [force]]")
}

fn optional_single_arg(
    command: &str,
    args: &[&str],
    argument_hint: &str,
) -> Result<Option<String>, SlashCommandParseError> {
    match args {
        [] => Ok(None),
        [value] => Ok(Some((*value).to_string())),
        _ => Err(usage_error(command, argument_hint)),
    }
}

fn require_remainder(
    command: &str,
    remainder: Option<String>,
    argument_hint: &str,
) -> Result<String, SlashCommandParseError> {
    remainder.ok_or_else(|| usage_error(command, argument_hint))
}

fn parse_goal_command(remainder: Option<&str>) -> Result<GoalCommand, SlashCommandParseError> {
    let Some(raw) = remainder.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(GoalCommand::Status);
    };
    let words = shell_words(raw).map_err(|error| command_error(&error, "goal", GOAL_USAGE))?;
    let Some(first) = words.first().map(String::as_str) else {
        return Ok(GoalCommand::Status);
    };
    if words.len() == 1 {
        match first {
            "status" | "show" => return Ok(GoalCommand::Status),
            "verify" | "check" => return Ok(GoalCommand::Verify),
            "pause" => return Ok(GoalCommand::Pause),
            "resume" => return Ok(GoalCommand::Resume),
            "clear" | "reset" | "cancel" | "stop" | "off" | "none" | "--clear" | "삭제"
            | "해제" | "초기화" => return Ok(GoalCommand::Clear),
            "history" | "log" => return Ok(GoalCommand::History),
            _ => {}
        }
    }
    match first {
        "edit" => {
            let goal = words[1..].join(" ").trim().to_string();
            if goal.is_empty() {
                return Err(usage_error("goal", "edit <goal text>"));
            }
            Ok(GoalCommand::Edit { goal })
        }
        "start" | "set" => parse_goal_start(&words[1..]),
        _ => parse_goal_start(&words),
    }
}

const GOAL_USAGE: &str = "[status|verify|pause|resume|clear|history|edit <goal>|<goal> [--check <validator>|--until <command>] [--every <duration>] [--no-plan] [--max-turns <n>] [--token-budget <n>] [--allow-writes]]";

/// Reject a malformed `--check` value at parse time (`GoalCheck`): a typo in
/// a known prefix like `cargo:tests` would otherwise be demoted to a command
/// that can only fail, removing objective validation from the goal.
fn validate_goal_check(check: &str) -> Result<(), SlashCommandParseError> {
    if check.trim().is_empty() {
        return Err(usage_error("goal", GOAL_USAGE));
    }
    crate::goal_checks::GoalCheck::parse(check)
        .map(|_| ())
        .map_err(|usage| usage_error("goal", usage))
}

fn parse_goal_start(words: &[String]) -> Result<GoalCommand, SlashCommandParseError> {
    let mut options = GoalOptions::default();
    let mut goal_parts = Vec::new();
    let mut index = 0;
    while index < words.len() {
        let word = &words[index];
        if word == "--check" || word == "-c" {
            index += 1;
            let Some(check) = words.get(index) else {
                return Err(usage_error("goal", GOAL_USAGE));
            };
            validate_goal_check(check)?;
            options.checks.push(check.clone());
        } else if let Some(check) = word.strip_prefix("--check=") {
            validate_goal_check(check)?;
            options.checks.push(check.to_string());
        } else if word == "--max-turns" || word == "--turns" {
            index += 1;
            let Some(turns) = words.get(index) else {
                return Err(usage_error("goal", GOAL_USAGE));
            };
            options.max_turns = Some(parse_positive_u32("goal", turns, "max turns")?);
        } else if let Some(turns) = word.strip_prefix("--max-turns=") {
            options.max_turns = Some(parse_positive_u32("goal", turns, "max turns")?);
        } else if let Some(turns) = word.strip_prefix("--turns=") {
            options.max_turns = Some(parse_positive_u32("goal", turns, "max turns")?);
        } else if word == "--token-budget" {
            index += 1;
            let Some(value) = words.get(index) else {
                return Err(usage_error("goal", GOAL_USAGE));
            };
            options.token_budget =
                Some(u64::from(parse_positive_u32("goal", value, "token budget")?));
        } else if let Some(value) = word.strip_prefix("--token-budget=") {
            options.token_budget =
                Some(u64::from(parse_positive_u32("goal", value, "token budget")?));
        } else if word == "--allow-writes" {
            // Opt out of the unattended read-only default: this goal's action and
            // repair turns inherit the session's write permission. A bare boolean
            // flag with no value.
            options.allow_writes = true;
        } else if word == "--no-plan" {
            options.no_plan = true;
        } else if word == "--until" {
            index += 1;
            let Some(command) = words.get(index) else {
                return Err(usage_error("goal", GOAL_USAGE));
            };
            options.until = Some(command.clone());
        } else if let Some(command) = word.strip_prefix("--until=") {
            if command.trim().is_empty() {
                return Err(usage_error("goal", GOAL_USAGE));
            }
            options.until = Some(command.to_string());
        } else if word == "--every" {
            index += 1;
            let Some(duration) = words.get(index) else {
                return Err(usage_error("goal", GOAL_USAGE));
            };
            options.every = Some(parse_duration_spec(duration)?);
        } else if let Some(duration) = word.strip_prefix("--every=") {
            options.every = Some(parse_duration_spec(duration)?);
        } else {
            goal_parts.push(word.clone());
        }
        index += 1;
    }
    let goal = goal_parts.join(" ").trim().to_string();
    if goal.is_empty() {
        return Err(usage_error("goal", GOAL_USAGE));
    }
    if options.until.is_some() && !options.checks.is_empty() {
        return Err(command_error(
            "Choose --until or --check for one goal; they are alternate completion policies.",
            "goal",
            GOAL_USAGE,
        ));
    }
    Ok(GoalCommand::Start { goal, options })
}

/// The overnight-protocol footer appended to every recipe preset template: each
/// iteration must leave a "what I did / what awaits your decision" record in the
/// team inbox `digest` channel (the morning digest) and push any must-read
/// finding to the user. Kept as a shared constant so the three presets stay in
/// lockstep.
const PRESET_OVERNIGHT_PROTOCOL: &str = "Overnight protocol: at the end of each iteration, record (a) what you did and (b) what is awaiting the user's decision via TeamInboxPost (channel: \"digest\", source: \"zo-loop\"). Push any finding the user must see verbatim to the user with send_to_user. These loops run read-only by default: do not edit files — leave changes as proposals.";

/// A `/loop` recipe preset: a curated recurring loop the user starts with a
/// single alias (`/loop ci`, `/loop pr`, `/loop audit`). Each carries a default
/// interval and a prompt template ending in [`PRESET_OVERNIGHT_PROTOCOL`]. The
/// presets are read-only by default (the host's unattended permission gate forces
/// read-only + propose-only); the user still opts into writes with `--allow-writes`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LoopPreset {
    Ci,
    Pr,
    Audit,
}

impl LoopPreset {
    fn from_token(token: &str) -> Option<Self> {
        match token {
            "ci" => Some(Self::Ci),
            "pr" => Some(Self::Pr),
            "audit" => Some(Self::Audit),
            _ => None,
        }
    }

    /// Default recurring interval for this preset.
    fn interval(self) -> &'static str {
        match self {
            Self::Ci => "5m",
            Self::Pr => "10m",
            Self::Audit => "30m",
        }
    }

    /// The curated body of the preset's prompt (the overnight-protocol footer is
    /// appended by [`Self::into_command`]).
    fn template_body(self) -> &'static str {
        match self {
            Self::Ci => "Check the latest CI status with `gh run list` and `gh run view`. If a run failed, read the failing logs and diagnose the root cause. Do NOT write code fixes — write up the diagnosis and a proposed fix as a proposal and record it via TeamInboxPost (channel: \"digest\"). If CI is green, record a brief green note instead.",
            Self::Pr => "Collect unanswered review comments on the open PRs with `gh api repos/{owner}/{repo}/pulls/<n>/comments` (resolve owner/repo from `gh repo view`). For each unresolved comment, draft a reply — do NOT post it. Record the drafts via TeamInboxPost (channel: \"digest\").",
            Self::Audit => "Audit the workspace: gather cargo clippy/test signals, and scan for dead code, TODO/FIXME markers, and suspicious patterns. Record findings ordered by severity via TeamInboxPost (channel: \"digest\").",
        }
    }

    /// Expand the preset (plus any user modifiers) into a recurring interval loop.
    /// Modifiers such as `--max-runs 10` are placed BEFORE the template so the
    /// host's `split_loop_budget_flags` (which only consumes leading flags)
    /// still parses them; the template's prose then ends the flag scan.
    fn into_command(self, modifiers: &str) -> Result<LoopCommand, SlashCommandParseError> {
        let template = format!("{} {PRESET_OVERNIGHT_PROTOCOL}", self.template_body());
        let modifiers = modifiers.trim();
        let prompt = if modifiers.is_empty() {
            template
        } else {
            format!("{modifiers} {template}")
        };
        Ok(LoopCommand::StartInterval {
            every: parse_duration_spec(self.interval())?,
            prompt,
        })
    }
}

fn parse_loop_command(remainder: Option<&str>) -> Result<LoopCommand, SlashCommandParseError> {
    let Some(raw) = remainder.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(LoopCommand::List);
    };
    // Only the leading sub-command/count token is structured. A loop PROMPT is
    // free text — it may contain apostrophes (`don't`), quotes, backslashes, and
    // runs of spaces — so it is taken verbatim and is NEVER shell-tokenized
    // (which used to turn `/loop 3 don't stop` into an "unclosed quote" usage
    // error). A single pair of wrapping quotes around the prompt is stripped for
    // ergonomics; everything else is preserved exactly.
    //
    // When the leading token is neither a recognized sub-command nor a count,
    // the entire remainder is a dynamic loop prompt. Its first turn is
    // immediate; later wakeups come from the turn-scoped `loop_schedule` tool.
    let (first, rest) = split_first_word(raw);
    // Recipe presets expand a single alias (`/loop ci|pr|audit [modifiers…]`) to a
    // curated recurring interval loop before the generic sub-command table below.
    if let Some(preset) = LoopPreset::from_token(first) {
        return preset.into_command(rest);
    }
    match first {
        "list" | "ls" => Ok(LoopCommand::List),
        "status" | "show" => Ok(LoopCommand::Status {
            id: first_token_opt(rest),
        }),
        "every" | "interval" => {
            let (every, prompt) = split_first_word(rest);
            let prompt = unquote(prompt).to_string();
            if every.is_empty() || prompt.is_empty() {
                return Err(usage_error("loop", LOOP_USAGE));
            }
            Ok(LoopCommand::StartInterval {
                every: parse_duration_spec(every)?,
                prompt,
            })
        }
        "watch" => {
            // The glob may itself be quoted (e.g. `'crates/**/*.rs'`) so it is
            // taken as a quote-aware token; the remainder is the verbatim prompt.
            let (glob, prompt_rest) = take_token(rest);
            let prompt = unquote(prompt_rest).to_string();
            if glob.trim().is_empty() || prompt.is_empty() {
                return Err(usage_error("loop", LOOP_USAGE));
            }
            Ok(LoopCommand::StartWatch { glob, prompt })
        }
        "until" => {
            let (check, prompt_rest) = take_token(rest);
            let prompt = unquote(prompt_rest).to_string();
            if check.trim().is_empty() || prompt.is_empty() {
                return Err(usage_error("loop", LOOP_USAGE));
            }
            Ok(LoopCommand::StartUntil { check, prompt })
        }
        "run" | "run-now" | "now" => Ok(LoopCommand::RunNow {
            id: first_token_opt(rest),
        }),
        "pause" => Ok(LoopCommand::Pause {
            id: first_token_opt(rest),
        }),
        "resume" => Ok(LoopCommand::Resume {
            id: first_token_opt(rest),
        }),
        "stop" | "cancel" => Ok(LoopCommand::Stop {
            all: rest
                .split_whitespace()
                .any(|word| word == "--all" || word == "all"),
            id: rest
                .split_whitespace()
                .find(|word| *word != "--all" && *word != "all")
                .map(str::to_string),
        }),
        "clear" => Ok(LoopCommand::Clear),
        count if !count.is_empty() && count.chars().all(|ch| ch.is_ascii_digit()) => {
            let count = parse_positive_u32("loop", count, "count")?;
            let prompt = unquote(rest).to_string();
            if prompt.is_empty() {
                return Err(usage_error("loop", LOOP_USAGE));
            }
            Ok(LoopCommand::StartFixedCount { count, prompt })
        }
        // No leading sub-command or count: keep the first word and use dynamic
        // scheduling after the immediate first iteration.
        _ => {
            let prompt = unquote(raw).to_string();
            if prompt.is_empty() {
                return Err(usage_error("loop", LOOP_USAGE));
            }
            Ok(LoopCommand::StartDynamic { prompt })
        }
    }
}

/// Split off the first whitespace-delimited token, returning `(token, rest)`
/// with `rest` left-trimmed. Used for structured leading tokens (sub-command,
/// count, duration) that never contain quotes; the caller keeps `rest` verbatim.
fn split_first_word(input: &str) -> (&str, &str) {
    let s = input.trim_start();
    match s.find(char::is_whitespace) {
        Some(idx) => (&s[..idx], s[idx..].trim_start()),
        None => (s, ""),
    }
}

/// First whitespace token of `rest` as an optional owned id (for status/run/
/// pause/resume, whose argument is a single bare id).
fn first_token_opt(rest: &str) -> Option<String> {
    rest.split_whitespace().next().map(str::to_string)
}

/// Take the first token honoring a leading `'...'`/`"..."` quoted run (quotes
/// removed, inner spaces kept); otherwise a whitespace-delimited word. Returns
/// `(token, rest)` with `rest` left-trimmed. An unclosed quote is lenient: the
/// remainder becomes the token rather than an error.
fn take_token(input: &str) -> (String, &str) {
    let s = input.trim_start();
    let mut chars = s.char_indices();
    if let Some((_, q)) = chars.next().filter(|&(_, c)| c == '\'' || c == '"') {
        for (i, c) in chars {
            if c == q {
                return (s[1..i].to_string(), s[i + c.len_utf8()..].trim_start());
            }
        }
        return (s[1..].to_string(), "");
    }
    match s.find(char::is_whitespace) {
        Some(idx) => (s[..idx].to_string(), s[idx..].trim_start()),
        None => (s.to_string(), ""),
    }
}

/// Strip exactly one pair of wrapping quotes from a free-text value when the
/// whole string is a single quoted span (so `"check CI"` → `check CI`), while
/// leaving prose with internal quotes/apostrophes untouched (`don't stop`,
/// `say "hi"`).
fn unquote(value: &str) -> &str {
    let s = value.trim();
    let first = s.chars().next();
    let last = s.chars().next_back();
    if let (Some(q @ ('"' | '\'')), Some(l)) = (first, last) {
        if q == l && s.len() >= 2 {
            let inner = &s[1..s.len() - 1];
            if !inner.contains(q) {
                return inner;
            }
        }
    }
    s
}

const LOOP_USAGE: &str = "[list|status [id]|ci|pr|audit [modifiers]|<count> <prompt>|<prompt>|every <duration> <prompt>|watch <glob> <prompt>|until <command> <prompt>|run [id]|pause [id]|resume [id]|stop [id|--all]|clear] (presets ci/pr/audit run read-only + propose; add --allow-writes to inherit session permission)";

fn parse_positive_u32(
    command: &str,
    value: &str,
    label: &str,
) -> Result<u32, SlashCommandParseError> {
    let parsed = value
        .parse::<u32>()
        .map_err(|_| command_error(&format!("Invalid {label}: `{value}`."), command, ""))?;
    if parsed == 0 {
        return Err(command_error(
            &format!("Invalid {label}: must be greater than zero."),
            command,
            "",
        ));
    }
    Ok(parsed)
}

fn parse_duration_spec(value: &str) -> Result<DurationSpec, SlashCommandParseError> {
    let duration = parse_duration(value).ok_or_else(|| {
        command_error(
            &format!("Invalid duration `{value}`. Use values like 30s, 5m, 1h, or 1d."),
            "loop",
            LOOP_USAGE,
        )
    })?;
    if duration.is_zero() {
        return Err(command_error(
            "Interval must be greater than zero.",
            "loop",
            LOOP_USAGE,
        ));
    }
    Ok(DurationSpec {
        raw: value.to_string(),
        duration,
    })
}

fn parse_duration(value: &str) -> Option<Duration> {
    let mut total = Duration::ZERO;
    let mut digits = String::new();
    let mut saw_unit = false;
    for ch in value.chars() {
        if ch.is_ascii_digit() {
            digits.push(ch);
            continue;
        }
        if digits.is_empty() {
            return None;
        }
        let amount = digits.parse::<u64>().ok()?;
        digits.clear();
        let seconds = match ch {
            's' | 'S' => amount,
            'm' | 'M' => amount.checked_mul(60)?,
            'h' | 'H' => amount.checked_mul(60 * 60)?,
            'd' | 'D' => amount.checked_mul(24 * 60 * 60)?,
            _ => return None,
        };
        total = total.checked_add(Duration::from_secs(seconds))?;
        saw_unit = true;
    }
    if !digits.is_empty() || !saw_unit || total.is_zero() {
        return None;
    }
    Some(total)
}

fn shell_words(input: &str) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    let mut current = String::new();
    let mut chars = input.chars().peekable();
    let mut quote: Option<char> = None;
    while let Some(ch) = chars.next() {
        match ch {
            c if quote == Some(c) => quote = None,
            '\\' => {
                if let Some(next) = chars.next() {
                    current.push(next);
                } else {
                    current.push('\\');
                }
            }
            // Only `"` groups tokens. A bare `'` is a literal apostrophe so a
            // natural-language argument (`fix it's broken`) no longer trips an
            // "unclosed quote" error; explicit grouping uses double quotes.
            '"' if quote.is_none() => quote = Some(ch),
            c if quote.is_none() && c.is_whitespace() => {
                if !current.is_empty() {
                    words.push(std::mem::take(&mut current));
                }
            }
            c => current.push(c),
        }
    }
    if let Some(q) = quote {
        return Err(format!("Unclosed quote `{q}` in slash command."));
    }
    if !current.is_empty() {
        words.push(current);
    }
    Ok(words)
}

/// Parse the optional `/share` target. Only a bare `/share` (local artifact) or
/// `/share gist` (hosted upload) are valid; any other token is rejected loudly
/// so a typo like `/share publik` never silently falls back to a local write.
fn parse_share_target(
    command: &str,
    args: &[&str],
) -> Result<Option<String>, SlashCommandParseError> {
    match args {
        [] => Ok(None),
        [target] if target.eq_ignore_ascii_case("gist") => Ok(Some("gist".to_string())),
        _ => Err(usage_error(command, "[gist]")),
    }
}

fn parse_permissions_mode(args: &[&str]) -> Result<Option<String>, SlashCommandParseError> {
    let mode = optional_single_arg(
        "permissions",
        args,
        "[read-only|workspace-write|danger-full-access]",
    )?;
    if let Some(mode) = mode {
        if matches!(
            mode.as_str(),
            "read-only" | "workspace-write" | "danger-full-access"
        ) {
            return Ok(Some(mode));
        }
        return Err(command_error(
            &format!(
                "Unsupported /permissions mode '{mode}'. Use read-only, workspace-write, or danger-full-access."
            ),
            "permissions",
            "/permissions [read-only|workspace-write|danger-full-access]",
        ));
    }

    Ok(None)
}

fn parse_clear_args(args: &[&str]) -> Result<bool, SlashCommandParseError> {
    match args {
        [] => Ok(false),
        ["--confirm"] => Ok(true),
        [unexpected] => Err(command_error(
            &format!("Unsupported /clear argument '{unexpected}'. Use /clear or /clear --confirm."),
            "clear",
            "/clear [--confirm]",
        )),
        _ => Err(usage_error("clear", "[--confirm]")),
    }
}

fn parse_config_section(args: &[&str]) -> Result<Option<String>, SlashCommandParseError> {
    let section = optional_single_arg("config", args, "[env|hooks|model|plugins]")?;
    if let Some(section) = section {
        if matches!(section.as_str(), "env" | "hooks" | "model" | "plugins") {
            return Ok(Some(section));
        }
        return Err(command_error(
            &format!("Unsupported /config section '{section}'. Use env, hooks, model, or plugins."),
            "config",
            "/config [env|hooks|model|plugins]",
        ));
    }

    Ok(None)
}

fn parse_session_command(args: &[&str]) -> Result<SlashCommand, SlashCommandParseError> {
    match args {
        [] => Ok(SlashCommand::Session {
            action: None,
            target: None,
        }),
        ["list"] => Ok(SlashCommand::Session {
            action: Some("list".to_string()),
            target: None,
        }),
        ["list", ..] => Err(usage_error(
            "session",
            "[list|switch <session-id>|fork [branch-name]]",
        )),
        ["switch"] => Err(usage_error("session switch", "<session-id>")),
        ["switch", target] => Ok(SlashCommand::Session {
            action: Some("switch".to_string()),
            target: Some((*target).to_string()),
        }),
        ["switch", ..] => Err(command_error(
            "Unexpected arguments for /session switch.",
            "session",
            "/session switch <session-id>",
        )),
        ["fork"] => Ok(SlashCommand::Session {
            action: Some("fork".to_string()),
            target: None,
        }),
        ["fork", target] => Ok(SlashCommand::Session {
            action: Some("fork".to_string()),
            target: Some((*target).to_string()),
        }),
        ["fork", ..] => Err(command_error(
            "Unexpected arguments for /session fork.",
            "session",
            "/session fork [branch-name]",
        )),
        [action, ..] => Err(command_error(
            &format!(
                "Unknown /session action '{action}'. Use list, switch <session-id>, or fork [branch-name]."
            ),
            "session",
            "/session [list|switch <session-id>|fork [branch-name]]",
        )),
    }
}

fn parse_name_command(name: Option<String>) -> Result<SlashCommand, SlashCommandParseError> {
    let Some(name) = name else {
        return Ok(SlashCommand::Name { name: None });
    };
    let name = name.trim();
    if name.is_empty() {
        return Err(usage_error("name", "<name>"));
    }
    if name.chars().count() > MAX_SESSION_NAME_CHARS {
        return Err(command_error(
            &format!("Session names are limited to {MAX_SESSION_NAME_CHARS} characters."),
            "name",
            "/name <name>",
        ));
    }
    Ok(SlashCommand::Name {
        name: Some(name.to_string()),
    })
}

fn parse_mcp_command(args: &[&str]) -> Result<SlashCommand, SlashCommandParseError> {
    match McpAction::parse(args) {
        Ok(action) => {
            let (action, target) = action.into_slash_parts();
            Ok(SlashCommand::Mcp { action, target })
        }
        Err(error) => Err(command_error(&error.message(), "mcp", error.usage())),
    }
}

fn parse_plugin_command(args: &[&str]) -> Result<SlashCommand, SlashCommandParseError> {
    match args {
        [] => Ok(SlashCommand::Plugins {
            action: None,
            target: None,
        }),
        ["list"] => Ok(SlashCommand::Plugins {
            action: Some("list".to_string()),
            target: None,
        }),
        ["list", ..] => Err(usage_error("plugin list", "")),
        ["install"] => Err(usage_error("plugin install", "<path>")),
        ["install", target @ ..] => Ok(SlashCommand::Plugins {
            action: Some("install".to_string()),
            target: Some(target.join(" ")),
        }),
        ["enable"] => Err(usage_error("plugin enable", "<name>")),
        ["enable", target] => Ok(SlashCommand::Plugins {
            action: Some("enable".to_string()),
            target: Some((*target).to_string()),
        }),
        ["enable", ..] => Err(command_error(
            "Unexpected arguments for /plugin enable.",
            "plugin",
            "/plugin enable <name>",
        )),
        ["disable"] => Err(usage_error("plugin disable", "<name>")),
        ["disable", target] => Ok(SlashCommand::Plugins {
            action: Some("disable".to_string()),
            target: Some((*target).to_string()),
        }),
        ["disable", ..] => Err(command_error(
            "Unexpected arguments for /plugin disable.",
            "plugin",
            "/plugin disable <name>",
        )),
        ["uninstall"] => Err(usage_error("plugin uninstall", "<id>")),
        ["uninstall", target] => Ok(SlashCommand::Plugins {
            action: Some("uninstall".to_string()),
            target: Some((*target).to_string()),
        }),
        ["uninstall", ..] => Err(command_error(
            "Unexpected arguments for /plugin uninstall.",
            "plugin",
            "/plugin uninstall <id>",
        )),
        ["update"] => Err(usage_error("plugin update", "<id>")),
        ["update", target] => Ok(SlashCommand::Plugins {
            action: Some("update".to_string()),
            target: Some((*target).to_string()),
        }),
        ["update", ..] => Err(command_error(
            "Unexpected arguments for /plugin update.",
            "plugin",
            "/plugin update <id>",
        )),
        [action, ..] => Err(command_error(
            &format!(
                "Unknown /plugin action '{action}'. Use list, install <path>, enable <name>, disable <name>, uninstall <id>, or update <id>."
            ),
            "plugin",
            "/plugin [list|install <path>|enable <name>|disable <name>|uninstall <id>|update <id>]",
        )),
    }
}

fn parse_list_or_help_args(
    command: &str,
    args: Option<String>,
) -> Result<Option<String>, SlashCommandParseError> {
    match normalize_optional_args(args.as_deref()) {
        None | Some("list" | "help" | "-h" | "--help") => Ok(args),
        Some(unexpected) => Err(command_error(
            &format!(
                "Unexpected arguments for /{command}: {unexpected}. Use /{command}, /{command} list, or /{command} help."
            ),
            command,
            &format!("/{command} [list|help]"),
        )),
    }
}

fn parse_skills_args(args: Option<&str>) -> Result<Option<String>, SlashCommandParseError> {
    let Some(args) = normalize_optional_args(args) else {
        return Ok(None);
    };

    if matches!(args, "list" | "help" | "-h" | "--help") {
        return Ok(Some(args.to_string()));
    }

    if args == "install" {
        return Err(command_error(
            "Usage: /skills install <path>",
            "skills",
            "/skills install <path>",
        ));
    }

    if let Some(target) = args.strip_prefix("install").map(str::trim) {
        if !target.is_empty() {
            return Ok(Some(format!("install {target}")));
        }
    }

    Err(command_error(
        &format!(
            "Unexpected arguments for /skills: {args}. Use /skills, /skills list, /skills install <path>, or /skills help."
        ),
        "skills",
        "/skills [list|install <path>|help]",
    ))
}

fn usage_error(command: &str, argument_hint: &str) -> SlashCommandParseError {
    let usage = format!("/{command} {argument_hint}");
    let usage = usage.trim_end().to_string();
    command_error(
        &format!("Usage: {usage}"),
        command_root_name(command),
        &usage,
    )
}

fn command_error(message: &str, command: &str, usage: &str) -> SlashCommandParseError {
    let detail = render_slash_command_help_detail(command)
        .map(|detail| format!("\n\n{detail}"))
        .unwrap_or_default();
    SlashCommandParseError::new(format!("{message}\n  Usage            {usage}{detail}"))
}

pub(crate) fn remainder_after_command(input: &str, command: &str) -> Option<String> {
    input
        .trim()
        .strip_prefix(&format!("/{command}"))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

pub(crate) fn normalize_optional_args(args: Option<&str>) -> Option<&str> {
    args.map(str::trim).filter(|value| !value.is_empty())
}

fn command_root_name(command: &str) -> &str {
    command.split_whitespace().next().unwrap_or(command)
}

#[cfg(test)]
mod loop_budget_tests {
    use std::time::Duration;

    use super::{GoalCommand, LoopCommand, SlashCommand, parse_loop_command};

    #[test]
    fn fixed_count_policy_is_left_to_the_autonomy_limit_table() {
        let parsed = SlashCommand::parse("/loop 51 bounded work")
            .expect("parse should succeed")
            .expect("slash command expected");
        assert_eq!(
            parsed,
            SlashCommand::Loop {
                command: LoopCommand::StartFixedCount {
                    count: 51,
                    prompt: "bounded work".to_string(),
                }
            }
        );

        assert!(parse_loop_command(Some("51 too many runs")).is_ok());
    }

    #[test]
    fn recurring_interval_accepts_the_one_second_e2e_cadence() {
        for spec in ["1s", "5m", "1h"] {
            let parsed = SlashCommand::parse(&format!("/loop every {spec} keep working"))
                .expect("parse should succeed")
                .expect("slash command expected");
            assert!(
                matches!(
                    parsed,
                    SlashCommand::Loop {
                        command: LoopCommand::StartInterval { .. }
                    }
                ),
                "`every {spec}` must be accepted"
            );
        }
    }

    #[test]
    fn bare_prompt_selects_dynamic_scheduling() {
        assert_eq!(
            SlashCommand::parse("/loop monitor CI")
                .expect("parse")
                .expect("command"),
            SlashCommand::Loop {
                command: LoopCommand::StartDynamic {
                    prompt: "monitor CI".to_string(),
                }
            }
        );
    }

    #[test]
    fn goal_policy_flags_do_not_leak_into_the_objective() {
        let parsed = SlashCommand::parse(
            "/goal deploy safely --until \"test -f ready\" --every 30s --no-plan",
        )
        .expect("parse")
        .expect("command");
        let SlashCommand::Goal {
            command: GoalCommand::Start { goal, options },
        } = parsed
        else {
            panic!("expected goal start");
        };
        assert_eq!(goal, "deploy safely");
        assert_eq!(options.until.as_deref(), Some("test -f ready"));
        assert_eq!(options.every.map(|every| every.duration), Some(Duration::from_secs(30)));
        assert!(options.no_plan);
    }

    #[test]
    fn until_loop_keeps_the_condition_separate_from_the_prompt() {
        assert_eq!(
            SlashCommand::parse("/loop until 'test -f ready' announce deployment")
                .expect("parse")
                .expect("command"),
            SlashCommand::Loop {
                command: LoopCommand::StartUntil {
                    check: "test -f ready".to_string(),
                    prompt: "announce deployment".to_string(),
                }
            }
        );
    }
}

#[cfg(test)]
mod compact_tests {
    use super::SlashCommand;

    #[test]
    fn compact_carries_optional_focus_instructions() {
        // CC parity: `/compact <focus>` steers what the summary preserves, while
        // a bare `/compact` keeps the default behavior (no instructions).
        let with_instructions = SlashCommand::parse("/compact focus on auth")
            .expect("parse should succeed")
            .expect("slash command expected");
        assert_eq!(
            with_instructions,
            SlashCommand::Compact {
                instructions: Some("focus on auth".to_string()),
            }
        );

        let bare = SlashCommand::parse("/compact")
            .expect("parse should succeed")
            .expect("slash command expected");
        assert_eq!(bare, SlashCommand::Compact { instructions: None });
    }
}

#[cfg(test)]
mod session_name_tests {
    use super::{SlashCommand, parse_name_command};

    #[test]
    fn name_parses_set_and_show_forms() {
        assert_eq!(
            SlashCommand::parse("/name   배포 관찰  "),
            Ok(Some(SlashCommand::Name {
                name: Some("배포 관찰".to_string()),
            }))
        );
        assert_eq!(
            SlashCommand::parse("/name"),
            Ok(Some(SlashCommand::Name { name: None }))
        );
    }

    #[test]
    fn name_rejects_too_long_and_empty_set_values() {
        let too_long = SlashCommand::parse(&format!("/name {}", "가".repeat(25)))
            .expect_err("25-character name must be rejected");
        assert!(too_long.to_string().contains("limited to 24 characters"));

        let empty = parse_name_command(Some("   ".to_string()))
            .expect_err("empty name must be rejected");
        assert!(empty.to_string().contains("Usage: /name <name>"));
    }
}

#[cfg(test)]
mod self_improve_tests {
    use super::{SelfImproveAction, SlashCommand};

    #[test]
    fn refine_parses_bare_and_rejects_arguments() {
        let refine = SlashCommand::parse("/refine")
            .expect("parse should succeed")
            .expect("slash command expected");
        assert_eq!(refine, SlashCommand::Refine);
        assert!(
            SlashCommand::parse("/refine now").is_err(),
            "/refine takes no arguments"
        );
    }

    #[test]
    fn improve_parses_status_apply_and_default_actions() {
        let status = SlashCommand::parse("/improve status")
            .expect("parse should succeed")
            .expect("slash command expected");
        assert_eq!(
            status,
            SlashCommand::SelfImprove {
                action: SelfImproveAction::Status,
            }
        );

        let apply = SlashCommand::parse(
        "/improve apply 0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
    )
    .expect("parse should succeed")
    .expect("slash command expected");
    assert_eq!(
        apply,
        SlashCommand::SelfImprove {
            action: SelfImproveAction::Apply {
                patch_digest: "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
                    .to_string(),
            },
        }
    );
    assert!(SlashCommand::parse("/improve apply").is_err());

        let propose = SlashCommand::parse("/improve")
            .expect("parse should succeed")
            .expect("slash command expected");
        assert_eq!(
            propose,
            SlashCommand::SelfImprove {
                action: SelfImproveAction::Propose,
            }
        );

        // Review-first lifecycle actions select an exact 64-hex proposal ID.
        let id = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";
        for (verb, expected) in [
            (
                "show",
                SelfImproveAction::Show {
                    proposal_id: id.to_string(),
                },
            ),
            (
                "review",
                SelfImproveAction::Review {
                    proposal_id: id.to_string(),
                },
            ),
            (
                "reject",
                SelfImproveAction::Reject {
                    proposal_id: id.to_string(),
                },
            ),
        ] {
            let parsed = SlashCommand::parse(&format!("/improve {verb} {id}"))
                .expect("parse should succeed")
                .expect("slash command expected");
            assert_eq!(parsed, SlashCommand::SelfImprove { action: expected });
            assert!(
                SlashCommand::parse(&format!("/improve {verb}")).is_err(),
                "{verb} without an ID must be a usage error"
            );
            assert!(
                SlashCommand::parse(&format!("/improve {verb} nothex")).is_err(),
                "{verb} with a non-hex ID must be a usage error"
            );
        }
    }
}
