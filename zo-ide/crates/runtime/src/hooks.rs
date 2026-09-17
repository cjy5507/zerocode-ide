use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::{
    atomic::{AtomicBool, AtomicU8, Ordering},
    Arc,
};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use crate::config::{RuntimeFeatureConfig, RuntimeHookConfig};
use crate::permissions::PermissionOverride;

const DEFAULT_HOOK_TIMEOUT: Duration = Duration::from_secs(120);
#[cfg(unix)]
const HOOK_PROCESS_GROUP_GRACE: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookEvent {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
    SessionStart,
    SessionEnd,
    UserPromptSubmit,
    PreCompact,
    PostCompact,
    SubagentStart,
    SubagentStop,
    TurnStart,
    TurnEnd,
    PermissionRequest,
    PermissionDenied,
    CwdChanged,
    /// A todo item appeared in the plan that was not there at turn entry (CC
    /// parity). The three task/file events below are what lets something
    /// OUTSIDE the process follow a long autonomous run without polling it —
    /// the reason to carry them at all.
    TaskCreated,
    /// A todo item reached `completed`.
    TaskCompleted,
    /// A file was created, modified, or deleted by a tool in this turn.
    FileChanged,
    /// User-attention moments (CC parity): fired when the harness surfaces a
    /// notification-worthy event — e.g. a permission prompt is displayed.
    Notification,
}

impl HookEvent {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
            Self::SessionStart => "SessionStart",
            Self::SessionEnd => "SessionEnd",
            Self::UserPromptSubmit => "UserPromptSubmit",
            Self::PreCompact => "PreCompact",
            Self::PostCompact => "PostCompact",
            Self::SubagentStart => "SubagentStart",
            Self::SubagentStop => "SubagentStop",
            Self::TurnStart => "TurnStart",
            Self::TurnEnd => "TurnEnd",
            Self::PermissionRequest => "PermissionRequest",
            Self::PermissionDenied => "PermissionDenied",
            Self::CwdChanged => "CwdChanged",
            Self::TaskCreated => "TaskCreated",
            Self::TaskCompleted => "TaskCompleted",
            Self::FileChanged => "FileChanged",
            Self::Notification => "Notification",
        }
    }
}

impl std::fmt::Display for HookEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HookProgressEvent {
    Started {
        event: HookEvent,
        tool_name: String,
        command: String,
    },
    Completed {
        event: HookEvent,
        tool_name: String,
        command: String,
    },
    Cancelled {
        event: HookEvent,
        tool_name: String,
        command: String,
    },
}

pub trait HookProgressReporter {
    fn on_event(&mut self, event: &HookProgressEvent);
}

/// Forwards blocking-worker progress over a channel that the async turn drains
/// while the hook runs. Send failures are ignored after the turn drops its
/// reporter; a detached hook must not panic while shutting down.
#[derive(Debug)]
pub(crate) struct ChannelHookProgressReporter {
    sender: tokio::sync::mpsc::UnboundedSender<HookProgressEvent>,
}

impl ChannelHookProgressReporter {
    #[must_use]
    pub(crate) fn new(sender: tokio::sync::mpsc::UnboundedSender<HookProgressEvent>) -> Self {
        Self { sender }
    }
}

impl HookProgressReporter for ChannelHookProgressReporter {
    fn on_event(&mut self, event: &HookProgressEvent) {
        let _ = self.sender.send(event.clone());
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookAbortOrigin {
    User,
    Host,
}

const ABORT_ORIGIN_NONE: u8 = 0;
const ABORT_ORIGIN_USER: u8 = 1;
const ABORT_ORIGIN_HOST: u8 = 2;

#[derive(Debug, Default)]
struct HookAbortState {
    aborted: AtomicBool,
    origin: AtomicU8,
    handled: AtomicBool,
}

#[derive(Debug, Clone, Default)]
pub struct HookAbortSignal {
    state: Arc<HookAbortState>,
}

impl HookAbortSignal {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Request cancellation on behalf of the user. User origin wins if a host
    /// failure races with the same turn stop.
    pub fn abort(&self) {
        self.state
            .origin
            .store(ABORT_ORIGIN_USER, Ordering::SeqCst);
        self.state.aborted.store(true, Ordering::SeqCst);
    }

    /// Request cancellation because the host stopped independently of the user.
    pub fn abort_host(&self) {
        let _ = self.state.origin.compare_exchange(
            ABORT_ORIGIN_NONE,
            ABORT_ORIGIN_HOST,
            Ordering::SeqCst,
            Ordering::SeqCst,
        );
        self.state.aborted.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_aborted(&self) -> bool {
        self.state.aborted.load(Ordering::SeqCst)
    }

    #[must_use]
    pub fn origin(&self) -> Option<HookAbortOrigin> {
        match self.state.origin.load(Ordering::SeqCst) {
            ABORT_ORIGIN_USER => Some(HookAbortOrigin::User),
            ABORT_ORIGIN_HOST => Some(HookAbortOrigin::Host),
            _ => None,
        }
    }

    /// Mark that the runtime has already persisted and rolled back this stop.
    pub fn mark_handled(&self) {
        self.state.handled.store(true, Ordering::SeqCst);
    }

    #[must_use]
    pub fn is_handled(&self) -> bool {
        self.state.handled.load(Ordering::SeqCst)
    }

    /// Borrow the underlying flag for cooperative cancel polling (e.g. a
    /// cancellable retry sleep). Lets generic helpers observe abort without
    /// depending on this type.
    #[must_use]
    pub fn flag(&self) -> &AtomicBool {
        &self.state.aborted
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HookRunResult {
    denied: bool,
    failed: bool,
    cancelled: bool,
    messages: Vec<String>,
    additional_context: Vec<String>,
    denial_reason: Option<String>,
    permission_override: Option<PermissionOverride>,
    permission_reason: Option<String>,
    updated_input: Option<String>,
    followup: Option<String>,
}

impl HookRunResult {
    #[must_use]
    pub fn empty() -> Self {
        Self::allow(Vec::new())
    }

    /// A hard failure carrying `message`. Used when a hook could not run to a
    /// verdict — e.g. the async off-task worker panicked — so the turn treats it
    /// as a failure (pre-hook denies the tool, post-hook marks the result an
    /// error) rather than silently allowing, which would bypass the hook policy.
    #[must_use]
    pub(crate) fn failed(message: String) -> Self {
        Self {
            failed: true,
            messages: vec![message],
            ..Self::allow(Vec::new())
        }
    }

    #[must_use]
    pub fn allow(messages: Vec<String>) -> Self {
        Self {
            denied: false,
            failed: false,
            cancelled: false,
            messages,
            additional_context: Vec::new(),
            denial_reason: None,
            permission_override: None,
            permission_reason: None,
            updated_input: None,
            followup: None,
        }
    }

    #[must_use]
    pub fn is_denied(&self) -> bool {
        self.denied
    }

    #[must_use]
    pub fn is_failed(&self) -> bool {
        self.failed
    }

    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.cancelled
    }

    #[must_use]
    pub fn messages(&self) -> &[String] {
        &self.messages
    }

    #[must_use]
    pub fn additional_context_messages(&self) -> &[String] {
        &self.additional_context
    }

    #[must_use]
    pub fn denial_reason(&self) -> Option<&str> {
        self.denial_reason.as_deref()
    }

    #[must_use]
    pub fn permission_override(&self) -> Option<PermissionOverride> {
        self.permission_override
    }

    #[must_use]
    pub fn permission_reason(&self) -> Option<&str> {
        self.permission_reason.as_deref()
    }

    #[must_use]
    pub fn updated_input(&self) -> Option<&str> {
        self.updated_input.as_deref()
    }

    /// Continuation message a `TurnEnd` (Stop) hook asked the agent to keep
    /// working on. When present, the turn loop re-injects it as the next user
    /// message and runs another turn (bounded by `max_stop_loops`), enabling
    /// goal-driven "keep going until done" loops.
    #[must_use]
    pub fn followup(&self) -> Option<&str> {
        self.followup.as_deref()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HookRunner {
    config: RuntimeHookConfig,
    /// `(agent_id, agent_type)` merged into every hook payload when this
    /// runner belongs to a spawned sub-agent (CC parity: hooks firing inside
    /// a sub-agent receive `agent_id`/`agent_type` so a user hook can tell
    /// sub-agent tool calls from main-agent ones). `None` on the main runtime.
    agent_context: Option<(String, String)>,
    /// User-declared `settings.env`, injected into every hook subprocess so a
    /// hook command sees the same session environment as the bash tool (CC
    /// parity). Empty unless built via [`Self::from_feature_config`].
    env: BTreeMap<String, String>,
}

impl HookRunner {
    #[must_use]
    pub fn new(config: RuntimeHookConfig) -> Self {
        Self {
            config,
            agent_context: None,
            env: BTreeMap::new(),
        }
    }

    /// Tag this runner as belonging to a sub-agent; see `agent_context`.
    pub fn set_agent_context(
        &mut self,
        agent_id: impl Into<String>,
        agent_type: impl Into<String>,
    ) {
        self.agent_context = Some((agent_id.into(), agent_type.into()));
    }

    #[must_use]
    pub fn from_feature_config(feature_config: &RuntimeFeatureConfig) -> Self {
        Self {
            config: feature_config.hooks().clone(),
            agent_context: None,
            env: feature_config.env().clone(),
        }
    }

    #[must_use]
    pub fn run_pre_tool_use(&self, tool_name: &str, tool_input: &str) -> HookRunResult {
        self.run_pre_tool_use_with_context(tool_name, tool_input, None, None)
    }

    #[must_use]
    pub fn run_pre_tool_use_with_context(
        &self,
        tool_name: &str,
        tool_input: &str,
        abort_signal: Option<&HookAbortSignal>,
        reporter: Option<&mut dyn HookProgressReporter>,
    ) -> HookRunResult {
        self.run_commands(
            HookEvent::PreToolUse,
            &self
                .config
                .matching_commands(HookEvent::PreToolUse, Some(tool_name)),
            tool_name,
            tool_input,
            None,
            false,
            abort_signal,
            reporter,
        )
    }

    #[must_use]
    pub fn run_post_tool_use_with_context(
        &self,
        tool_name: &str,
        tool_input: &str,
        tool_output: &str,
        is_error: bool,
        abort_signal: Option<&HookAbortSignal>,
        reporter: Option<&mut dyn HookProgressReporter>,
    ) -> HookRunResult {
        self.run_commands(
            HookEvent::PostToolUse,
            &self
                .config
                .matching_commands(HookEvent::PostToolUse, Some(tool_name)),
            tool_name,
            tool_input,
            Some(tool_output),
            is_error,
            abort_signal,
            reporter,
        )
    }

    #[must_use]
    pub fn run_post_tool_use_failure(
        &self,
        tool_name: &str,
        tool_input: &str,
        tool_error: &str,
    ) -> HookRunResult {
        self.run_post_tool_use_failure_with_context(tool_name, tool_input, tool_error, None, None)
    }

    #[must_use]
    pub fn run_post_tool_use_failure_with_context(
        &self,
        tool_name: &str,
        tool_input: &str,
        tool_error: &str,
        abort_signal: Option<&HookAbortSignal>,
        reporter: Option<&mut dyn HookProgressReporter>,
    ) -> HookRunResult {
        self.run_commands(
            HookEvent::PostToolUseFailure,
            &self
                .config
                .matching_commands(HookEvent::PostToolUseFailure, Some(tool_name)),
            tool_name,
            tool_input,
            Some(tool_error),
            true,
            abort_signal,
            reporter,
        )
    }

    #[must_use]
    pub fn run_lifecycle_event(&self, event: HookEvent, context: &Value) -> HookRunResult {
        // Tool-agnostic lifecycle events (SessionStart, UserPromptSubmit, …)
        // have no tool name to match against, so every rule's command runs.
        let commands = self.config.matching_commands(event, None);
        if commands.is_empty() {
            return HookRunResult::empty();
        }
        let context_str = context.to_string();
        self.run_commands(
            event,
            &commands,
            event.as_str(),
            &context_str,
            None,
            false,
            None,
            None,
        )
    }

    #[must_use]
    pub fn lifecycle_command_count(&self, event: HookEvent) -> usize {
        self.config.matching_commands(event, None).len()
    }

    #[allow(clippy::too_many_arguments)]
    fn run_commands(
        &self,
        event: HookEvent,
        commands: &[String],
        tool_name: &str,
        tool_input: &str,
        tool_output: Option<&str>,
        is_error: bool,
        abort_signal: Option<&HookAbortSignal>,
        mut reporter: Option<&mut dyn HookProgressReporter>,
    ) -> HookRunResult {
        if commands.is_empty() {
            return HookRunResult::allow(Vec::new());
        }

        if abort_signal.is_some_and(HookAbortSignal::is_aborted) {
            return HookRunResult {
                denied: false,
                failed: false,
                cancelled: true,
                messages: vec![format!(
                    "{} hook cancelled before execution",
                    event.as_str()
                )],
                additional_context: Vec::new(),
                denial_reason: None,
                permission_override: None,
                permission_reason: None,
                updated_input: None,
                followup: None,
            };
        }

        let mut payload = hook_payload(event, tool_name, tool_input, tool_output, is_error);
        if let Some((agent_id, agent_type)) = &self.agent_context {
            payload["agent_id"] = serde_json::Value::String(agent_id.clone());
            payload["agent_type"] = serde_json::Value::String(agent_type.clone());
        }
        let payload = payload.to_string();
        let mut result = HookRunResult::allow(Vec::new());

        for command in commands {
            if let Some(reporter) = reporter.as_deref_mut() {
                reporter.on_event(&HookProgressEvent::Started {
                    event,
                    tool_name: tool_name.to_string(),
                    command: command.clone(),
                });
            }

            match Self::run_command(
                command,
                event,
                tool_name,
                tool_input,
                tool_output,
                is_error,
                &payload,
                abort_signal,
                self.config.hook_timeout().unwrap_or(DEFAULT_HOOK_TIMEOUT),
                &self.env,
            ) {
                HookCommandOutcome::Allow { parsed } => {
                    if let Some(reporter) = reporter.as_deref_mut() {
                        reporter.on_event(&HookProgressEvent::Completed {
                            event,
                            tool_name: tool_name.to_string(),
                            command: command.clone(),
                        });
                    }
                    merge_parsed_hook_output(&mut result, parsed);
                }
                HookCommandOutcome::Deny { parsed } => {
                    if let Some(reporter) = reporter.as_deref_mut() {
                        reporter.on_event(&HookProgressEvent::Completed {
                            event,
                            tool_name: tool_name.to_string(),
                            command: command.clone(),
                        });
                    }
                    merge_parsed_hook_output(&mut result, parsed);
                    result.denied = true;
                    return result;
                }
                HookCommandOutcome::Failed { parsed } => {
                    if let Some(reporter) = reporter.as_deref_mut() {
                        reporter.on_event(&HookProgressEvent::Completed {
                            event,
                            tool_name: tool_name.to_string(),
                            command: command.clone(),
                        });
                    }
                    merge_parsed_hook_output(&mut result, parsed);
                    result.failed = true;
                    return result;
                }
                HookCommandOutcome::Cancelled { message } => {
                    if let Some(reporter) = reporter.as_deref_mut() {
                        reporter.on_event(&HookProgressEvent::Cancelled {
                            event,
                            tool_name: tool_name.to_string(),
                            command: command.clone(),
                        });
                    }
                    result.cancelled = true;
                    result.messages.push(message);
                    return result;
                }
            }
        }

        result
    }

    #[allow(clippy::too_many_arguments)]
    fn run_command(
        command: &str,
        event: HookEvent,
        tool_name: &str,
        tool_input: &str,
        tool_output: Option<&str>,
        is_error: bool,
        payload: &str,
        abort_signal: Option<&HookAbortSignal>,
        timeout: Duration,
        env: &BTreeMap<String, String>,
    ) -> HookCommandOutcome {
        let mut child = shell_command(command);
        child.stdin(Stdio::piped());
        child.stdout(Stdio::piped());
        child.stderr(Stdio::piped());
        // User settings.env first, so the zo-internal HOOK_* payload vars
        // below always win over any collision.
        child.envs(env);
        child.env("HOOK_EVENT", event.as_str());
        child.env("HOOK_TOOL_NAME", tool_name);
        child.env("HOOK_TOOL_INPUT", tool_input);
        child.env("HOOK_TOOL_IS_ERROR", if is_error { "1" } else { "0" });
        if let Some(tool_output) = tool_output {
            // Truncate to 64 KB to avoid exceeding OS environment size limits.
            const HOOK_OUTPUT_MAX: usize = 64 * 1024;
            let truncated = if tool_output.len() > HOOK_OUTPUT_MAX {
                &tool_output[..tool_output.floor_char_boundary(HOOK_OUTPUT_MAX)]
            } else {
                tool_output
            };
            child.env("HOOK_TOOL_OUTPUT", truncated);
        }

        match child.output_with_stdin(payload.as_bytes(), abort_signal, timeout) {
            Ok(CommandExecution::Finished(output)) => {
                let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                let parsed = parse_hook_output(&stdout);
                let primary_message = parsed.primary_message().map(ToOwned::to_owned);
                match output.status.code() {
                    Some(0) => {
                        if parsed.deny {
                            HookCommandOutcome::Deny { parsed }
                        } else {
                            HookCommandOutcome::Allow { parsed }
                        }
                    }
                    Some(2) => HookCommandOutcome::Deny {
                        parsed: parsed.with_fallback_message(format!(
                            "{} hook denied tool `{tool_name}`",
                            event.as_str()
                        )),
                    },
                    Some(code) => HookCommandOutcome::Failed {
                        parsed: parsed.with_fallback_message(format_hook_failure(
                            command,
                            code,
                            primary_message.as_deref(),
                            stderr.as_str(),
                        )),
                    },
                    None => HookCommandOutcome::Failed {
                        parsed: parsed.with_fallback_message(format!(
                            "{} hook `{command}` terminated by signal while handling `{}`",
                            event.as_str(),
                            tool_name
                        )),
                    },
                }
            }
            Ok(CommandExecution::Cancelled) => HookCommandOutcome::Cancelled {
                message: format!(
                    "{} hook `{command}` cancelled while handling `{tool_name}`",
                    event.as_str()
                ),
            },
            Ok(CommandExecution::TimedOut { timeout }) => HookCommandOutcome::Failed {
                parsed: ParsedHookOutput {
                    messages: vec![format!(
                        "{} hook `{command}` timed out after {:.1}s while handling `{tool_name}`",
                        event.as_str(),
                        timeout.as_secs_f64()
                    )],
                    ..ParsedHookOutput::default()
                },
            },
            Err(error) => HookCommandOutcome::Failed {
                parsed: ParsedHookOutput {
                    messages: vec![format!(
                        "{} hook `{command}` failed to start for `{}`: {error}",
                        event.as_str(),
                        tool_name
                    )],
                    ..ParsedHookOutput::default()
                },
            },
        }
    }
}

enum HookCommandOutcome {
    Allow { parsed: ParsedHookOutput },
    Deny { parsed: ParsedHookOutput },
    Failed { parsed: ParsedHookOutput },
    Cancelled { message: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct ParsedHookOutput {
    messages: Vec<String>,
    additional_context: Vec<String>,
    denial_reason: Option<String>,
    deny: bool,
    permission_override: Option<PermissionOverride>,
    permission_reason: Option<String>,
    updated_input: Option<String>,
    followup: Option<String>,
}

impl ParsedHookOutput {
    fn with_fallback_message(mut self, fallback: String) -> Self {
        if self.messages.is_empty() {
            self.messages.push(fallback);
        }
        self
    }

    fn primary_message(&self) -> Option<&str> {
        self.messages.first().map(String::as_str)
    }
}

fn merge_parsed_hook_output(target: &mut HookRunResult, parsed: ParsedHookOutput) {
    target.messages.extend(parsed.messages);
    target.additional_context.extend(parsed.additional_context);
    if parsed.denial_reason.is_some() {
        target.denial_reason = parsed.denial_reason;
    }
    if parsed.permission_override.is_some() {
        target.permission_override = parsed.permission_override;
    }
    if parsed.permission_reason.is_some() {
        target.permission_reason = parsed.permission_reason;
    }
    if parsed.updated_input.is_some() {
        target.updated_input = parsed.updated_input;
    }
    if parsed.followup.is_some() {
        target.followup = parsed.followup;
    }
}

fn parse_hook_output(stdout: &str) -> ParsedHookOutput {
    if stdout.is_empty() {
        return ParsedHookOutput::default();
    }

    let Ok(Value::Object(root)) = serde_json::from_str::<Value>(stdout) else {
        return ParsedHookOutput {
            messages: vec![stdout.to_string()],
            ..ParsedHookOutput::default()
        };
    };

    let mut parsed = ParsedHookOutput::default();

    if let Some(message) = root.get("systemMessage").and_then(Value::as_str) {
        parsed.messages.push(message.to_string());
    }
    if let Some(message) = root.get("reason").and_then(Value::as_str) {
        parsed.messages.push(message.to_string());
        parsed.denial_reason = Some(message.to_string());
    }
    if root.get("continue").and_then(Value::as_bool) == Some(false)
        || root.get("decision").and_then(Value::as_str) == Some("block")
    {
        parsed.deny = true;
    }
    // A `TurnEnd` (Stop) hook can ask the agent to keep working by returning a
    // `followupMessage`, accepted either at the root or under
    // `hookSpecificOutput` (the convention shared with the other camelCase
    // hook keys below).
    if let Some(followup) = root.get("followupMessage").and_then(Value::as_str) {
        parsed.followup = Some(followup.to_string());
    }

    if let Some(Value::Object(specific)) = root.get("hookSpecificOutput") {
        // `additionalContext` is PROMPT MATERIAL, not a message. It rides
        // `additional_context` alone and never `messages`: `messages` is what a
        // hook SAYS — it is rendered (a denial banner, an error cell) and read
        // by a person, and a hook that both blocks and contributes context used
        // to have its whole context paragraph printed as the refusal.
        if let Some(Value::String(additional_context)) = specific.get("additionalContext") {
            parsed.additional_context.push(additional_context.clone());
        }
        if let Some(decision) = specific.get("permissionDecision").and_then(Value::as_str) {
            parsed.permission_override = match decision {
                "allow" => Some(PermissionOverride::Allow),
                "deny" => Some(PermissionOverride::Deny),
                "ask" => Some(PermissionOverride::Ask),
                _ => None,
            };
        }
        if let Some(reason) = specific
            .get("permissionDecisionReason")
            .and_then(Value::as_str)
        {
            parsed.permission_reason = Some(reason.to_string());
        }
        if let Some(updated_input) = specific.get("updatedInput") {
            parsed.updated_input = serde_json::to_string(updated_input).ok();
        }
        if let Some(followup) = specific.get("followupMessage").and_then(Value::as_str) {
            parsed.followup = Some(followup.to_string());
        }
    }

    // Don't echo the raw JSON as a feedback message when the hook only carried
    // a structured directive (e.g. a bare `followupMessage`, or a bare
    // `additionalContext` — the shape a context-contributing hook has).
    if parsed.messages.is_empty() && parsed.followup.is_none() && parsed.additional_context.is_empty()
    {
        parsed.messages.push(stdout.to_string());
    }

    parsed
}

fn hook_payload(
    event: HookEvent,
    tool_name: &str,
    tool_input: &str,
    tool_output: Option<&str>,
    is_error: bool,
) -> Value {
    match event {
        HookEvent::PostToolUseFailure => json!({
            "hook_event_name": event.as_str(),
            "tool_name": tool_name,
            "tool_input": parse_tool_input(tool_input),
            "tool_input_json": tool_input,
            "tool_error": tool_output,
            "tool_result_is_error": true,
        }),
        _ => json!({
            "hook_event_name": event.as_str(),
            "tool_name": tool_name,
            "tool_input": parse_tool_input(tool_input),
            "tool_input_json": tool_input,
            "tool_output": tool_output,
            "tool_result_is_error": is_error,
        }),
    }
}

fn parse_tool_input(tool_input: &str) -> Value {
    serde_json::from_str(tool_input).unwrap_or_else(|_| json!({ "raw": tool_input }))
}

fn format_hook_failure(command: &str, code: i32, stdout: Option<&str>, stderr: &str) -> String {
    let mut message = format!("Hook `{command}` exited with status {code}");
    if let Some(stdout) = stdout.filter(|stdout| !stdout.is_empty()) {
        message.push_str(": ");
        message.push_str(stdout);
    } else if !stderr.is_empty() {
        message.push_str(": ");
        message.push_str(stderr);
    }
    message
}

fn shell_command(command: &str) -> CommandWithStdin {
    #[cfg(windows)]
    let command_builder = {
        let mut command_builder = Command::new("cmd");
        command_builder.arg("/C").arg(command);
        CommandWithStdin::new(command_builder)
    };

    #[cfg(not(windows))]
    let command_builder = {
        let mut command_builder = Command::new("sh");
        command_builder.arg("-lc").arg(command);
        CommandWithStdin::new(command_builder)
    };

    command_builder
}

struct CommandWithStdin {
    command: Command,
}

impl CommandWithStdin {
    fn new(command: Command) -> Self {
        Self { command }
    }

    fn stdin(&mut self, cfg: Stdio) -> &mut Self {
        self.command.stdin(cfg);
        self
    }

    fn stdout(&mut self, cfg: Stdio) -> &mut Self {
        self.command.stdout(cfg);
        self
    }

    fn stderr(&mut self, cfg: Stdio) -> &mut Self {
        self.command.stderr(cfg);
        self
    }

    fn env<K, V>(&mut self, key: K, value: V) -> &mut Self
    where
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.command.env(key, value);
        self
    }

    fn envs<I, K, V>(&mut self, vars: I) -> &mut Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<OsStr>,
        V: AsRef<OsStr>,
    {
        self.command.envs(vars);
        self
    }

    #[cfg(windows)]
    fn output_with_stdin(
        &mut self,
        stdin: &[u8],
        abort_signal: Option<&HookAbortSignal>,
        timeout: Duration,
    ) -> std::io::Result<CommandExecution> {
        use process_wrap::std::{CommandWrap, JobObject};
        use std::io::Read;

        // A Windows Job Object is the process-tree equivalent of the Unix
        // process group below. Membership is inherited by descendants, and
        // `start_kill` terminates the entire job, so cancellation/timeout does
        // not strand a shell grandchild with our pipe handles open.
        let placeholder = Command::new("cmd");
        let command = std::mem::replace(&mut self.command, placeholder);
        let mut wrapped = CommandWrap::from(command);
        wrapped.wrap(JobObject);
        let mut child = wrapped.spawn()?;

        let stdin_writer = child.stdin().take().map(|mut pipe| {
            let payload = stdin.to_vec();
            thread::spawn(move || {
                let _ = pipe.write_all(&payload);
            })
        });
        let stdout_reader = child.stdout().take().map(|mut pipe| {
            thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = pipe.read_to_end(&mut buf);
                buf
            })
        });
        let stderr_reader = child.stderr().take().map(|mut pipe| {
            thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = pipe.read_to_end(&mut buf);
                buf
            })
        });

        let started = Instant::now();
        let mut timed_out = false;
        let mut poll_error = None;
        let exit_status = loop {
            if abort_signal.is_some_and(HookAbortSignal::is_aborted) {
                let _ = child.kill();
                break None;
            }
            if started.elapsed() >= timeout {
                timed_out = true;
                let _ = child.kill();
                break None;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    // The command shell may exit before a descendant. Closing
                    // the job here terminates any remainder before pipe joins.
                    let _ = child.start_kill();
                    let _ = child.wait();
                    break Some(status);
                }
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(error) => {
                    let _ = child.kill();
                    poll_error = Some(error);
                    break None;
                }
            }
        };

        if let Some(writer) = stdin_writer {
            let _ = writer.join();
        }
        let stdout = stdout_reader
            .map(|reader| reader.join().unwrap_or_default())
            .unwrap_or_default();
        let stderr = stderr_reader
            .map(|reader| reader.join().unwrap_or_default())
            .unwrap_or_default();

        if let Some(error) = poll_error {
            return Err(error);
        }
        match exit_status {
            Some(status) => Ok(CommandExecution::Finished(std::process::Output {
                status,
                stdout,
                stderr,
            })),
            None if timed_out => Ok(CommandExecution::TimedOut { timeout }),
            None => Ok(CommandExecution::Cancelled),
        }
    }

    #[cfg(not(any(unix, windows)))]
    fn output_with_stdin(
        &mut self,
        stdin: &[u8],
        abort_signal: Option<&HookAbortSignal>,
        timeout: Duration,
    ) -> std::io::Result<CommandExecution> {
        let _ = (stdin, abort_signal, timeout);
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "hook subprocesses are disabled without safe process-tree cleanup",
        ))
    }

    #[cfg(unix)]
    fn output_with_stdin(
        &mut self,
        stdin: &[u8],
        abort_signal: Option<&HookAbortSignal>,
        timeout: Duration,
    ) -> std::io::Result<CommandExecution> {
        use std::io::Read;
        use std::os::unix::process::CommandExt;

        self.command.process_group(0);

        let mut child = self.command.spawn()?;

        // stdin write 와 stdout/stderr read 를 각각 별도 스레드로 분리한다.
        // 단일 스레드로 write_all 후 poll 하면 두 겹의 파이프 데드락이 난다:
        // ① 대용량 stdin 이 파이프 버퍼를 넘으면 write_all 이 블록되고, ②
        // poll loop 가 자식 종료 전까지 stdout 을 비우지 않으므로, 자식이
        // 대용량 출력을 내면 stdout 파이프가 가득 차 자식이 멈춰 try_wait 이
        // 영영 None 이 된다. 세 파이프를 동시에 진행시키면 교착이 사라지고,
        // 부모는 try_wait 폴링으로 abort 응답성을 유지한다.
        let stdin_writer = child.stdin.take().map(|mut pipe| {
            let payload = stdin.to_vec();
            thread::spawn(move || {
                // 자식이 stdin 을 다 안 읽고 끝나면 EPIPE 가 날 수 있으나
                // 무시한다. 스코프 종료 시 pipe drop → EOF 전달.
                let _ = pipe.write_all(&payload);
            })
        });
        let stdout_reader = child.stdout.take().map(|mut pipe| {
            thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = pipe.read_to_end(&mut buf);
                buf
            })
        });
        let stderr_reader = child.stderr.take().map(|mut pipe| {
            thread::spawn(move || {
                let mut buf = Vec::new();
                let _ = pipe.read_to_end(&mut buf);
                buf
            })
        });

        // The direct child is not the only process that can retain these pipes:
        // a shell hook may have already exited after spawning descendants. Always
        // terminate its group before joining drain threads, so inherited stdout,
        // stderr, or stdin descriptors cannot keep those joins blocked.
        let started = Instant::now();
        let mut timed_out = false;
        let mut poll_error = None;
        let exit_status: Option<std::process::ExitStatus> = loop {
            if abort_signal.is_some_and(HookAbortSignal::is_aborted) {
                terminate_hook_child(&mut child);
                break None;
            }
            if started.elapsed() >= timeout {
                timed_out = true;
                terminate_hook_child(&mut child);
                break None;
            }
            match child.try_wait() {
                Ok(Some(status)) => {
                    terminate_hook_process_group(child.id());
                    break Some(status);
                }
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(error) => {
                    terminate_hook_child(&mut child);
                    poll_error = Some(error);
                    break None;
                }
            }
        };

        // Group cleanup above closes any inherited descriptors before these
        // joins, so the drain and stdin writer threads cannot wait on a
        // descendant after the direct child has exited.
        if let Some(writer) = stdin_writer {
            let _ = writer.join();
        }
        let stdout = stdout_reader
            .map(|reader| reader.join().unwrap_or_default())
            .unwrap_or_default();
        let stderr = stderr_reader
            .map(|reader| reader.join().unwrap_or_default())
            .unwrap_or_default();

        if let Some(error) = poll_error {
            return Err(error);
        }

        match exit_status {
            Some(status) => Ok(CommandExecution::Finished(std::process::Output {
                status,
                stdout,
                stderr,
            })),
            None if timed_out => Ok(CommandExecution::TimedOut { timeout }),
            None => Ok(CommandExecution::Cancelled),
        }
    }
}

#[cfg(unix)]
fn terminate_hook_child(child: &mut std::process::Child) {
    terminate_hook_process_group(child.id());
    let _ = child.kill();
    let _ = child.wait();
}

#[cfg(unix)]
fn terminate_hook_process_group(pid: u32) {
    use nix::errno::Errno;
    use nix::sys::signal::{killpg, Signal};
    use nix::unistd::Pid;

    let Ok(pid) = i32::try_from(pid) else {
        return;
    };
    let process_group = Pid::from_raw(pid);
    let group_may_remain = matches!(
        killpg(process_group, Signal::SIGTERM),
        Ok(()) | Err(Errno::EPERM)
    );
    if group_may_remain {
        thread::sleep(HOOK_PROCESS_GROUP_GRACE);
        let _ = killpg(process_group, Signal::SIGKILL);
    }
}

enum CommandExecution {
    Finished(std::process::Output),
    Cancelled,
    TimedOut { timeout: Duration },
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::thread;
    use std::time::{Duration, Instant};

    use super::{
        shell_command, CommandExecution, HookAbortSignal, HookEvent, HookProgressEvent,
        HookProgressReporter, HookRunResult, HookRunner,
    };
    use crate::config::{RuntimeFeatureConfig, RuntimeHookConfig};
    use crate::permissions::PermissionOverride;

    struct RecordingReporter {
        events: Vec<HookProgressEvent>,
    }

    impl HookProgressReporter for RecordingReporter {
        fn on_event(&mut self, event: &HookProgressEvent) {
            self.events.push(event.clone());
        }
    }

    #[cfg(unix)]
    #[test]
    fn command_with_stdin_times_out_and_kills_term_ignoring_descendants() {
        let mut command = shell_command("trap '' TERM; sleep 5 & wait");
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let started = Instant::now();
        let result = command
            .output_with_stdin(&[], None, Duration::from_millis(30))
            .expect("timeout execution should return cleanly");

        assert!(matches!(result, CommandExecution::TimedOut { .. }));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "term-ignoring descendant held the pipe open for {:?}",
            started.elapsed()
        );
    }

    #[cfg(unix)]
    #[test]
    fn command_with_stdin_kills_descendants_after_successful_parent_exit() {
        let mut command = shell_command("sleep 5 & exit 0");
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let started = Instant::now();
        let result = command
            .output_with_stdin(&[], None, Duration::from_secs(1))
            .expect("successful parent execution should return cleanly");

        assert!(matches!(result, CommandExecution::Finished(output) if output.status.success()));
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "descendant held inherited output pipes open for {:?}",
            started.elapsed()
        );
    }

    #[cfg(windows)]
    #[test]
    fn command_with_stdin_times_out_and_terminates_windows_job() {
        let mut command = shell_command("ping -n 6 127.0.0.1 >nul");
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let started = Instant::now();
        let result = command
            .output_with_stdin(&[], None, Duration::from_millis(30))
            .expect("Windows Job timeout should return cleanly");
        assert!(matches!(result, CommandExecution::TimedOut { .. }));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[cfg(windows)]
    #[test]
    fn command_with_stdin_kills_windows_job_descendants_after_parent_exit() {
        let mut command = shell_command(
            r#"start "" /B cmd /C "ping -n 6 127.0.0.1 ^>nul" & exit 0"#,
        );
        command
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped());

        let started = Instant::now();
        let result = command
            .output_with_stdin(&[], None, Duration::from_secs(1))
            .expect("successful Windows parent execution should return cleanly");
        assert!(matches!(result, CommandExecution::Finished(output) if output.status.success()));
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn allows_exit_code_zero_and_captures_stdout() {
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![shell_snippet("printf 'pre ok'")],
            Vec::new(),
            Vec::new(),
        ));

        let result = runner.run_pre_tool_use("Read", r#"{"path":"README.md"}"#);

        assert_eq!(result, HookRunResult::allow(vec!["pre ok".to_string()]));
    }

    /// 서브에이전트 뷰: 메인 전용 이벤트(Stop/TurnEnd·UserPromptSubmit·Session*)는
    /// 제거되고 도구·서브에이전트 훅은 유지된다 — 사용자 Stop 게이트가
    /// 서브에이전트의 좁은 태스크를 재루프시키지 않는 CC 계약.
    #[test]
    fn subagent_view_strips_main_agent_only_hooks() {
        let config = RuntimeHookConfig::new(
            vec![shell_snippet("printf 'pre ok'")],
            Vec::new(),
            Vec::new(),
        )
        .with_turn_end(vec![shell_snippet(
            r#"printf '{"hookSpecificOutput":{"followupMessage":"loop"}}'"#,
        )])
        .with_subagent_lifecycle(
            vec!["echo start".to_string()],
            vec!["echo stop".to_string()],
        )
        .with_task_created(vec!["echo created".to_string()])
        .with_task_completed(vec!["echo completed".to_string()])
        .with_file_changed(vec!["echo wrote".to_string()])
        .with_cwd_changed(vec!["echo moved".to_string()]);

        let view = config.for_subagent();
        assert!(view.turn_end().is_empty());
        assert!(view.user_prompt_submit().is_empty());
        assert!(view.session_start().is_empty());
        assert_eq!(view.pre_tool_use().len(), 1, "tool hooks stay");
        // The todo plan is one session-level file every sub-agent sharing the
        // workspace can see, so N sub-agents would announce one completion N
        // times over.
        assert!(view.task_created().is_empty(), "the plan is session-level");
        assert!(view.task_completed().is_empty(), "the plan is session-level");
        // These read the sub-agent's own process state and own transcript, so
        // each reports only what it actually did.
        assert_eq!(view.file_changed().len(), 1, "its own edits are its own");
        assert_eq!(view.cwd_changed().len(), 1, "its own cwd is its own");
        assert_eq!(view.subagent_start().len(), 1, "subagent lifecycle stays");
        assert_eq!(view.subagent_stop().len(), 1);

        // Behavioral: the view's TurnEnd yields no followup → no Stop-loop fuel.
        let runner = HookRunner::new(view);
        let result = runner.run_lifecycle_event(HookEvent::TurnEnd, &serde_json::json!({}));
        assert_eq!(result.followup(), None);
    }

    /// 서브에이전트 러너의 모든 훅 페이로드에 `agent_id`/`agent_type` 이 실린다
    /// (CC 파리티: 훅 스크립트가 메인/서브 호출을 구분 가능).
    #[test]
    fn agent_context_rides_on_every_hook_payload() {
        let mut runner = HookRunner::new(RuntimeHookConfig::new(
            // `cat` echoes the stdin payload back as the hook message.
            vec![shell_snippet("cat")],
            Vec::new(),
            Vec::new(),
        ));
        runner.set_agent_context("agent-7", "Explore");

        let result = runner.run_pre_tool_use("Read", r#"{"path":"x"}"#);

        let echoed = result.messages().join("");
        assert!(echoed.contains(r#""agent_id":"agent-7""#), "{echoed}");
        assert!(echoed.contains(r#""agent_type":"Explore""#), "{echoed}");

        // The main runtime (no agent context) must stay byte-identical.
        let plain = HookRunner::new(RuntimeHookConfig::new(
            vec![shell_snippet("cat")],
            Vec::new(),
            Vec::new(),
        ));
        let result = plain.run_pre_tool_use("Read", r#"{"path":"x"}"#);
        assert!(!result.messages().join("").contains("agent_id"));
    }

    /// settings.env 는 훅 서브프로세스에도 주입된다(bash 툴과 동일 세션 환경,
    /// CC 파리티). 이전엔 `RuntimeFeatureConfig` 에 env 필드 자체가 없어 파싱된
    /// env 가 어떤 자식에도 닿지 않았다(사일런트 풋건).
    #[test]
    fn settings_env_is_injected_into_hook_subprocess() {
        // Echo the injected probe var back as the hook message.
        let probe = "printf 'saw:%s' \"$ZO_HOOK_ENV_PROBE\"";
        let feature = RuntimeFeatureConfig::default()
            .with_hooks(RuntimeHookConfig::new(
                vec![shell_snippet(probe)],
                Vec::new(),
                Vec::new(),
            ))
            .with_env(BTreeMap::from([(
                "ZO_HOOK_ENV_PROBE".to_string(),
                "injected".to_string(),
            )]));
        let runner = HookRunner::from_feature_config(&feature);
        let result = runner.run_pre_tool_use("Read", r#"{"path":"x"}"#);
        assert!(
            result.messages().join("").contains("saw:injected"),
            "hook subprocess must see settings.env; got {:?}",
            result.messages()
        );

        // A runner with no env (main `new`) leaves the uniquely-named probe unset,
        // so the expansion is empty — proving the value came from settings.env,
        // not the ambient process environment.
        let plain = HookRunner::new(RuntimeHookConfig::new(
            vec![shell_snippet(probe)],
            Vec::new(),
            Vec::new(),
        ));
        let echoed = plain.run_pre_tool_use("Read", r#"{"path":"x"}"#).messages().join("");
        assert!(!echoed.contains("saw:injected"), "unset probe must be empty; got {echoed:?}");
    }

    #[test]
    fn parses_dedicated_denial_reason_without_reordering_messages() {
        let parsed = super::parse_hook_output(
            r#"{"systemMessage":"banner","decision":"block","reason":"real reason"}"#,
        );

        assert!(parsed.deny);
        assert_eq!(parsed.messages, ["banner".to_string(), "real reason".to_string()]);
        assert_eq!(parsed.denial_reason.as_deref(), Some("real reason"));
    }

    /// `additionalContext` is prompt material and must never join `messages`.
    ///
    /// `messages` is what a hook SAYS: it is rendered as a denial banner, an
    /// error cell, the `⚠ denied '<tool>': …` line. When the context shared
    /// that vector, a hook that both blocked and contributed context printed
    /// its whole context paragraph on screen as the refusal — the model's
    /// prompt material read back to the person as an error.
    #[test]
    fn additional_context_never_becomes_a_rendered_hook_message() {
        let parsed = super::parse_hook_output(
            r#"{"systemMessage":"banner","reason":"root reason","hookSpecificOutput":{"additionalContext":"extra context"}}"#,
        );

        assert_eq!(
            parsed.messages,
            ["banner".to_string(), "root reason".to_string()]
        );
        assert_eq!(parsed.additional_context, ["extra context".to_string()]);
        assert_eq!(parsed.denial_reason.as_deref(), Some("root reason"));
    }

    /// A hook whose whole output IS context must not be echoed as raw JSON.
    ///
    /// The "nothing structured came back, so show the stdout" fallback counts
    /// `messages` and `followupMessage`; once the context left `messages` this
    /// shape looked empty, and the fallback would have printed the JSON — the
    /// same leak wearing a different coat.
    #[test]
    fn a_bare_additional_context_is_neither_a_message_nor_an_echo() {
        let parsed = super::parse_hook_output(
            r#"{"hookSpecificOutput":{"hookEventName":"UserPromptSubmit","additionalContext":"only context"}}"#,
        );

        assert!(parsed.messages.is_empty(), "{:?}", parsed.messages);
        assert_eq!(parsed.additional_context, ["only context".to_string()]);
        assert!(!parsed.deny);
    }

    /// The blocking shape the leak was observed through: `decision: block` plus
    /// context and no `reason`. The refusal a person sees must fall back to the
    /// runtime's own wording, not to the hook's prompt material.
    #[test]
    fn a_blocking_hook_with_only_context_carries_no_denial_reason() {
        let parsed = super::parse_hook_output(
            r#"{"decision":"block","hookSpecificOutput":{"additionalContext":"orchestration contract paragraph"}}"#,
        );

        assert!(parsed.deny);
        assert_eq!(parsed.denial_reason, None);
        assert!(parsed.messages.is_empty(), "{:?}", parsed.messages);
        assert_eq!(
            parsed.additional_context,
            ["orchestration contract paragraph".to_string()]
        );
    }

    #[test]
    fn merges_additional_context_from_multiple_hooks() {
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![
                shell_snippet(r#"printf '{"systemMessage":"banner one","hookSpecificOutput":{"additionalContext":"context one"}}'"#),
                shell_snippet(r#"printf '{"systemMessage":"banner two","hookSpecificOutput":{"additionalContext":"context two"}}'"#),
            ],
            Vec::new(),
            Vec::new(),
        ));

        let result = runner.run_pre_tool_use("Read", r#"{"path":"x"}"#);

        assert_eq!(
            result.messages(),
            &["banner one".to_string(), "banner two".to_string()]
        );
        assert_eq!(
            result.additional_context_messages(),
            &["context one".to_string(), "context two".to_string()]
        );
    }

    #[test]
    fn parses_followup_message_for_stop_loop() {
        let nested =
            super::parse_hook_output(r#"{"hookSpecificOutput":{"followupMessage":"keep going"}}"#);
        assert_eq!(nested.followup.as_deref(), Some("keep going"));
        // A bare followup directive must not leak as a feedback message.
        assert!(nested.messages.is_empty());

        let root = super::parse_hook_output(r#"{"followupMessage":"at root"}"#);
        assert_eq!(root.followup.as_deref(), Some("at root"));

        let none = super::parse_hook_output(r#"{"systemMessage":"hi"}"#);
        assert_eq!(none.followup, None);
    }

    #[test]
    fn turn_end_hook_surfaces_followup() {
        let runner =
            HookRunner::new(
                RuntimeHookConfig::default().with_turn_end(vec![shell_snippet(
                    r#"printf '{"hookSpecificOutput":{"followupMessage":"continue"}}'"#,
                )]),
            );

        let result =
            runner.run_lifecycle_event(HookEvent::TurnEnd, &serde_json::json!({"iterations": 1}));

        assert_eq!(result.followup(), Some("continue"));
    }

    #[test]
    fn denies_exit_code_two() {
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![shell_snippet("printf 'blocked by hook'; exit 2")],
            Vec::new(),
            Vec::new(),
        ));

        let result = runner.run_pre_tool_use("Bash", r#"{"command":"pwd"}"#);

        assert!(result.is_denied());
        assert_eq!(result.messages(), &["blocked by hook".to_string()]);
    }

    #[test]
    fn does_not_deadlock_on_large_stdin_with_large_stdout() {
        // C3 회귀: hook 이 stdin 을 읽지 않고 대용량 stdout(>파이프 버퍼)을
        // 내도 파이프 교착 없이 완료해야 한다. 구버전은 부모가 write_all 에
        // 블록된 채 자식 stdout 을 비우지 못해 영구 행(hang)이었다. 200KB
        // stdin + 256KB stdout 을 함께 유발해 양쪽 파이프를 모두 채운다.
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![shell_snippet("yes X | head -c 262144")],
            Vec::new(),
            Vec::new(),
        ));

        let big_input = format!(r#"{{"command":"{}"}}"#, "A".repeat(200_000));
        let result = runner.run_pre_tool_use("Bash", &big_input);

        // 교착 없이 정상 종료(exit 0)했으면 거부가 아니어야 한다.
        assert!(!result.is_denied());
    }

    #[test]
    fn propagates_other_non_zero_statuses_as_failures() {
        let runner = HookRunner::from_feature_config(&RuntimeFeatureConfig::default().with_hooks(
            RuntimeHookConfig::new(
                vec![shell_snippet("printf 'warning hook'; exit 1")],
                Vec::new(),
                Vec::new(),
            ),
        ));

        // given
        // when
        let result = runner.run_pre_tool_use("Edit", r#"{"file":"src/lib.rs"}"#);

        // then
        assert!(result.is_failed());
        assert!(result
            .messages()
            .iter()
            .any(|message| message.contains("warning hook")));
    }

    #[test]
    fn parses_pre_hook_permission_override_and_updated_input() {
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![shell_snippet(
                r#"printf '%s' '{"systemMessage":"updated","hookSpecificOutput":{"permissionDecision":"allow","permissionDecisionReason":"hook ok","updatedInput":{"command":"git status"}}}'"#,
            )],
            Vec::new(),
            Vec::new(),
        ));

        let result = runner.run_pre_tool_use("bash", r#"{"command":"pwd"}"#);

        assert_eq!(
            result.permission_override(),
            Some(PermissionOverride::Allow)
        );
        assert_eq!(result.permission_reason(), Some("hook ok"));
        assert_eq!(result.updated_input(), Some(r#"{"command":"git status"}"#));
        assert!(result.messages().iter().any(|message| message == "updated"));
    }

    #[test]
    fn runs_post_tool_use_failure_hooks() {
        // given
        let runner = HookRunner::new(RuntimeHookConfig::new(
            Vec::new(),
            Vec::new(),
            vec![shell_snippet("printf 'failure hook ran'")],
        ));

        // when
        let result =
            runner.run_post_tool_use_failure("bash", r#"{"command":"false"}"#, "command failed");

        // then
        assert!(!result.is_denied());
        assert_eq!(result.messages(), &["failure hook ran".to_string()]);
    }

    #[test]
    fn stops_running_failure_hooks_after_failure() {
        // given
        let runner = HookRunner::new(RuntimeHookConfig::new(
            Vec::new(),
            Vec::new(),
            vec![
                shell_snippet("printf 'broken failure hook'; exit 1"),
                shell_snippet("printf 'later failure hook'"),
            ],
        ));

        // when
        let result =
            runner.run_post_tool_use_failure("bash", r#"{"command":"false"}"#, "command failed");

        // then
        assert!(result.is_failed());
        assert!(result
            .messages()
            .iter()
            .any(|message| message.contains("broken failure hook")));
        assert!(!result
            .messages()
            .iter()
            .any(|message| message == "later failure hook"));
    }

    #[test]
    fn executes_hooks_in_configured_order() {
        // given
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![
                shell_snippet("printf 'first'"),
                shell_snippet("printf 'second'"),
            ],
            Vec::new(),
            Vec::new(),
        ));
        let mut reporter = RecordingReporter { events: Vec::new() };

        // when
        let result = runner.run_pre_tool_use_with_context(
            "Read",
            r#"{"path":"README.md"}"#,
            None,
            Some(&mut reporter),
        );

        // then
        assert_eq!(
            result,
            HookRunResult::allow(vec!["first".to_string(), "second".to_string()])
        );
        assert_eq!(reporter.events.len(), 4);
        assert!(matches!(
            &reporter.events[0],
            HookProgressEvent::Started {
                event: HookEvent::PreToolUse,
                command,
                ..
            } if command == &shell_snippet("printf 'first'")
        ));
        assert!(matches!(
            &reporter.events[1],
            HookProgressEvent::Completed {
                event: HookEvent::PreToolUse,
                command,
                ..
            } if command == &shell_snippet("printf 'first'")
        ));
        assert!(matches!(
            &reporter.events[2],
            HookProgressEvent::Started {
                event: HookEvent::PreToolUse,
                command,
                ..
            } if command == &shell_snippet("printf 'second'")
        ));
        assert!(matches!(
            &reporter.events[3],
            HookProgressEvent::Completed {
                event: HookEvent::PreToolUse,
                command,
                ..
            } if command == &shell_snippet("printf 'second'")
        ));
    }

    #[test]
    fn stops_running_hooks_after_failure() {
        // given
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![
                shell_snippet("printf 'broken'; exit 1"),
                shell_snippet("printf 'later'"),
            ],
            Vec::new(),
            Vec::new(),
        ));

        // when
        let result = runner.run_pre_tool_use("Edit", r#"{"file":"src/lib.rs"}"#);

        // then
        assert!(result.is_failed());
        assert!(result
            .messages()
            .iter()
            .any(|message| message.contains("broken")));
        assert!(!result.messages().iter().any(|message| message == "later"));
    }

    #[test]
    fn abort_signal_cancels_long_running_hook_and_reports_progress() {
        let runner = HookRunner::new(RuntimeHookConfig::new(
            vec![shell_snippet("sleep 5")],
            Vec::new(),
            Vec::new(),
        ));
        let abort_signal = HookAbortSignal::new();
        let abort_signal_for_thread = abort_signal.clone();
        let mut reporter = RecordingReporter { events: Vec::new() };

        thread::spawn(move || {
            thread::sleep(Duration::from_millis(100));
            abort_signal_for_thread.abort();
        });

        let result = runner.run_pre_tool_use_with_context(
            "bash",
            r#"{"command":"sleep 5"}"#,
            Some(&abort_signal),
            Some(&mut reporter),
        );

        assert!(result.is_cancelled());
        assert!(reporter.events.iter().any(|event| matches!(
            event,
            HookProgressEvent::Started {
                event: HookEvent::PreToolUse,
                ..
            }
        )));
        assert!(reporter.events.iter().any(|event| matches!(
            event,
            HookProgressEvent::Cancelled {
                event: HookEvent::PreToolUse,
                ..
            }
        )));
    }

    #[cfg(windows)]
    fn shell_snippet(script: &str) -> String {
        if script == "cat" {
            return "more".to_string();
        }
        if script == "sleep 5" {
            return "ping -n 6 127.0.0.1 >nul".to_string();
        }
        if script == "yes X | head -c 262144" {
            return "powershell -NoProfile -Command \"[Console]::Out.Write('X' * 262144)\""
                .to_string();
        }
        if script == "printf 'saw:%s' \"$ZO_HOOK_ENV_PROBE\"" {
            return r#"<nul set /p "=saw:%ZO_HOOK_ENV_PROBE%""#.to_string();
        }

        let (body, status) = script
            .rsplit_once("; exit ")
            .map_or((script, None), |(body, status)| (body, Some(status)));
        let payload = if let Some(rest) = body.strip_prefix("printf '%s' '") {
            rest.strip_suffix('\'')
        } else {
            body.strip_prefix("printf '")
                .and_then(|rest| rest.strip_suffix('\''))
        };
        if let Some(payload) = payload {
            let command = format!(r#"<nul set /p "={}""#, payload.replace('%', "%%"));
            return status.map_or(command.clone(), |status| format!("{command} & exit {status}"));
        }
        script.to_string()
    }

    #[cfg(not(windows))]
    fn shell_snippet(script: &str) -> String {
        script.to_string()
    }
}
