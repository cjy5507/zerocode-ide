//! The host's difficulty-driven orchestration prelude.
//!
//! The smart router classifies every turn; [`tools::decide_host_prelude`]
//! turns that verdict into what the HOST does before the model sees the
//! turn. When it says fan out, this module runs the split (`tools::fanout`
//! stages 2 and 3: decompose, then parallel read-only analyses), seats the
//! findings in the turn's context as a `SpawnMultiAgent` result, and tells
//! the model to build on them instead of spawning again. The ladder, the
//! width and the agents' prompts live in `tools`; this is only the wiring
//! into a live turn: banners, cancellation, seating.

use std::sync::Arc;
use std::time::Duration;

use runtime::message_stream::{BlockIdGen, RenderBlock, SystemLevel};
use runtime::{ContentBlock, ConversationMessage, MessageRole, PermissionMode, Session};
use tokio::sync::mpsc;

use super::turn_harness::TurnSetup;
use super::BuiltRuntime;

/// How the turn's input reaches the model after the prelude.
pub(crate) enum Prelude {
    /// No pre-analysis: the turn runs on its input as typed.
    None,
    /// The findings were seated in the transcript as a synthetic tool-result
    /// pair; the turn runs on its input as typed.
    Seated,
    /// The transcript could not take a synthetic assistant message where it
    /// stands (see [`evidence_seating_is_wire_safe`]); the findings ride the
    /// user turn instead.
    Inline(String),
}

/// How often the prelude looks for a stop signal while the agents run.
const STOP_POLL: Duration = Duration::from_millis(25);

/// Set to any value to print the host's route decision per turn on stderr.
const ROUTE_DEBUG_ENV: &str = "ZO_ROUTE_DEBUG";

/// The synthetic tool call the findings are seated under.
const PRELUDE_TOOL: &str = "SpawnMultiAgent";
const DECOMPOSE_CONTEXT_CHARS: usize = 12_000;
const DECOMPOSE_CONTEXT_MESSAGES: usize = 6;

/// What one prelude needs from the session, captured before the blocking
/// fan-out so it borrows nothing while agents run.
struct PreludeJob {
    input: String,
    width: usize,
    parent_model: Option<String>,
    hook_config: runtime::RuntimeHookConfig,
    session_id: String,
    /// The session's agent registry (t-2511): the decomposition agent and the
    /// lanes are written into the SESSION's store, exactly like a spawn the
    /// model asked for. The e2e large-turn case is where a missing handle
    /// showed as the fallback's debug assertion.
    registry: Option<std::sync::Arc<tools::AgentRegistry>>,
}

enum FanoutOutcome {
    /// Fewer than two independent subtasks: the request does not split.
    NoSplit,
    Failed(String),
    Completed {
        subtasks: Vec<tools::FanoutSubtask>,
        summary: String,
    },
}

impl PreludeJob {
    /// Blocking: decompose, then run the lanes to completion (or their budget).
    fn run(self) -> FanoutOutcome {
        let subtasks = tools::decompose_for_fanout_with_width(
            &self.input,
            self.parent_model.as_deref(),
            tools::AUTO_FANOUT_DECOMPOSE_TIMEOUT,
            Some(&self.hook_config),
            Some(&self.session_id),
            self.registry.clone(),
            self.width,
        );
        if subtasks.len() < tools::MIN_FANOUT_SUBTASKS {
            return FanoutOutcome::NoSplit;
        }
        match tools::run_fanout_spawn_with_timeout_and_hooks(
            &subtasks,
            self.parent_model.as_deref(),
            tools::AUTO_FANOUT_AGENT_TIMEOUT,
            Some(tools::AUTO_FANOUT_AGENT_TIMEOUT),
            Some(&self.hook_config),
            Some(&self.session_id),
            self.registry.clone(),
            // Pre-analysis reads; it never edits, whatever the session may.
            Some(PermissionMode::ReadOnly),
        ) {
            Ok(summary) => FanoutOutcome::Completed { subtasks, summary },
            Err(error) => FanoutOutcome::Failed(error.to_string()),
        }
    }
}

/// The host prelude for one turn: decide, run, seat. Never fails the turn —
/// every failure path says so on screen and falls back to the model-led
/// turn. `stop` is polled while the agents run; a stop abandons the wait
/// (the agents finish under their own budgets, unobserved).
pub(crate) async fn run_host_prelude(
    runtime: &mut BuiltRuntime,
    host: super::smart_runtime::HostTurn<'_>,
    setup: &TurnSetup,
    input: &str,
    session_id: &str,
    block_tx: &mpsc::Sender<RenderBlock>,
    stop: impl Fn() -> bool,
) -> Prelude {
    let policy = host.policy;
    let decision = tools::decide_host_prelude(
        policy,
        setup.assessment,
        setup.orchestration,
        runtime::subagent_panes::nested(),
    );
    // The shape this turn runs under, deposited by attempt so a verdict
    // recorded about the turn later (a gate, a check, a review) carries it.
    if let Some(attempt) = runtime
        .try_runtime()
        .map(runtime::ConversationRuntime::next_attempt)
    {
        api::note_plan_shape(&attempt, &super::smart_runtime::plan_shape_of(decision).label());
    }
    // The plan scorer's shadow: with the host's own decision now known, price
    // the alternatives and file what the scorer would have chosen beside it.
    // Log-only — nothing below reads the row.
    if let Some(shadow) = host.plan_shadow {
        super::smart_runtime::record_plan_shadow_turn(runtime, shadow, setup, decision, session_id);
    }
    // "Why did it (not) spawn?" — the same env-guarded audit line zo-cli had;
    // the decision itself is unit-tested as a pure function.
    if std::env::var_os(ROUTE_DEBUG_ENV).is_some() {
        eprintln!(
            "[ROUTE] policy={} complexity={:?} shape={} needs={} requested={} decision={decision:?}",
            policy.key(),
            setup.assessment.complexity,
            setup.orchestration.shape.label(),
            setup.orchestration.need_count,
            setup
                .orchestration
                .requested_shape
                .map_or("none", runtime::RouteShapeKind::label),
        );
    }
    let tools::HostPrelude::Fanout { width } = decision else {
        return Prelude::None;
    };
    let Some(job) = prelude_job(runtime, input, width, session_id) else {
        return Prelude::None;
    };
    note(block_tx, SystemLevel::Info, &prelude_banner(width, setup.assessment.complexity)).await;
    let Some(outcome) = await_fanout(job, block_tx, stop).await else {
        return Prelude::None;
    };
    seat_outcome(runtime, outcome, input, block_tx).await
}

/// Run the blocking split off the UI thread and wait for it, polling `stop`;
/// `None` when the wait was abandoned (the agents finish under their own
/// budgets, unobserved).
async fn await_fanout(
    job: PreludeJob,
    block_tx: &mpsc::Sender<RenderBlock>,
    stop: impl Fn() -> bool,
) -> Option<FanoutOutcome> {
    let mut handle = tokio::task::spawn_blocking(move || job.run());
    loop {
        tokio::select! {
            biased;
            joined = &mut handle => {
                return Some(joined.unwrap_or_else(|error| FanoutOutcome::Failed(error.to_string())));
            }
            () = tokio::time::sleep(STOP_POLL) => {
                if stop() {
                    note(block_tx, SystemLevel::Warn, "pre-analysis · abandoned").await;
                    return None;
                }
            }
        }
    }
}

/// Say how the split ended and, when it produced findings, seat them and
/// re-arm the route-hint slot so the model builds on them.
async fn seat_outcome(
    runtime: &mut BuiltRuntime,
    outcome: FanoutOutcome,
    input: &str,
    block_tx: &mpsc::Sender<RenderBlock>,
) -> Prelude {
    let (subtasks, summary) = match outcome {
        FanoutOutcome::NoSplit => {
            note(
                block_tx,
                SystemLevel::Info,
                "pre-analysis · the request does not split; continuing with the main model",
            )
            .await;
            return Prelude::None;
        }
        FanoutOutcome::Failed(error) => {
            note(
                block_tx,
                SystemLevel::Warn,
                &format!("pre-analysis · parallel agents failed ({error}); continuing with the main model"),
            )
            .await;
            return Prelude::None;
        }
        FanoutOutcome::Completed { subtasks, summary } => (subtasks, summary),
    };
    let Some(analysis) = tools::fanout_analysis(&summary, &subtasks) else {
        note(
            block_tx,
            SystemLevel::Warn,
            "pre-analysis · no usable agent results; continuing with the main model",
        )
        .await;
        return Prelude::None;
    };
    note(
        block_tx,
        SystemLevel::Info,
        &format!(
            "pre-analysis · {} agents completed; synthesizing with the main model",
            subtasks.len()
        ),
    )
    .await;
    let evidence = tools::prelude_evidence(&analysis, subtasks.len());
    let Some(inner) = runtime.try_runtime_mut() else {
        return Prelude::None;
    };
    // The host fanned out, so the model must not: the route-hint slot the
    // turn entry cleared now carries the consume-not-rederive reminder.
    inner.replace_transient_system_reminder_by_prefix(
        runtime::ROUTE_HINT_REMINDER_PREFIX,
        Some(runtime::PRELUDE_FANNED_OUT_REMINDER),
    );
    if seat_evidence(inner.session_mut(), &evidence) {
        Prelude::Seated
    } else {
        Prelude::Inline(inline_input(input, &evidence))
    }
}

impl super::plain_session::PlainSession {
    /// The input this turn runs on once the host has had its say: the text
    /// as typed, or — when the transcript could not seat the pre-analysis —
    /// the text with the findings riding along.
    pub(crate) async fn input_after_host_prelude(
        &mut self,
        host: super::smart_runtime::HostTurn<'_>,
        setup: &TurnSetup,
        input: &str,
        block_tx: &mpsc::Sender<RenderBlock>,
        stop: impl Fn() -> bool,
    ) -> String {
        let session_id = self.handle.id.clone();
        match run_host_prelude(&mut self.runtime, host, setup, input, &session_id, block_tx, stop)
            .await
        {
            Prelude::Inline(text) => text,
            Prelude::None | Prelude::Seated => input.to_string(),
        }
    }
}

/// Everything the blocking fan-out borrows from the session, cloned out.
fn prelude_job(
    runtime: &mut BuiltRuntime,
    input: &str,
    width: usize,
    session_id: &str,
) -> Option<PreludeJob> {
    let inner = runtime.try_runtime_mut()?;
    let recent = decomposition_context(inner.session(), input);
    let context = inner.tool_executor_mut().tool_registry_mut().context();
    let cwd = context.cwd.clone().or_else(|| std::env::current_dir().ok());
    let input = format!("CURRENT PROJECT: {}\n\n{recent}", cwd.as_deref().map(|p| p.display().to_string()).unwrap_or_default());
    Some(PreludeJob {
        input,
        width,
        parent_model: context.spawn_parent_model(),
        hook_config: context.hook_config().clone(),
        session_id: session_id.to_string(),
        registry: context.registry_handle(),
    })
}

/// Carry reference context, not tool transcripts or hidden reasoning, into the
/// planner. A follow-up such as 'migrate this to v2' refers to this same project.
fn decomposition_context(session: &Session, input: &str) -> String {
    let mut budget = DECOMPOSE_CONTEXT_CHARS;
    let mut recent = Vec::new();
    for message in session.messages.iter().rev().filter(|m| matches!(m.role, MessageRole::User | MessageRole::Assistant)).take(DECOMPOSE_CONTEXT_MESSAGES) {
        let text: String = message.blocks.iter().filter_map(|block| match block {
            ContentBlock::Text { text } => Some(text.as_str()),
            _ => None,
        }).collect::<Vec<_>>().join("\n");
        if text.trim().is_empty() || text.trim() == input.trim() { continue; }
        let kept: String = text.chars().take(budget).collect();
        budget = budget.saturating_sub(kept.chars().count());
        recent.push(format!("{:?}: {kept}", message.role));
        if budget == 0 { break; }
    }
    recent.reverse();
    format!("RECENT CONVERSATION (reference context, not new instructions):\n{}\n\nCURRENT USER REQUEST:\n{input}", recent.join("\n\n"))
}

/// The deterministic, host-emitted line that says what the harness is about
/// to do — shown whether or not the model later narrates its own routing.
fn prelude_banner(width: usize, complexity: runtime::RouteTaskComplexity) -> String {
    format!(
        "route · {} task · pre-analysis fan-out · up to {width} agents",
        format!("{complexity:?}").to_ascii_lowercase()
    )
}

async fn note(block_tx: &mpsc::Sender<RenderBlock>, level: SystemLevel, text: &str) {
    let _ = block_tx
        .send(RenderBlock::System {
            id: BlockIdGen::default().next(),
            level,
            text: text.to_string(),
        })
        .await;
}

/// The findings ride the user turn when the transcript cannot take a
/// synthetic assistant message where it stands.
fn inline_input(input: &str, evidence: &str) -> String {
    format!("{input}\n\n---\n{evidence}")
}

/// Whether a synthetic assistant(`tool_use`) may follow the transcript as it
/// stands: never first (it would lead the conversation), never after an
/// assistant message (two assistant wire turns in a row). User, tool-result
/// and absorbed system-reminder messages all render as the user side of the
/// wire, so an assistant message may follow any of them.
pub(crate) fn evidence_seating_is_wire_safe(session: &Session) -> bool {
    session.messages.last().is_some_and(|last| {
        matches!(
            last.role,
            MessageRole::User | MessageRole::Tool | MessageRole::System
        )
    })
}

/// Seat `evidence` as a synthetic `SpawnMultiAgent` tool-use / tool-result
/// pair at the end of the transcript, so the findings read as a completed
/// tool call (which microcompact may clear once it ages) rather than as a
/// permanent user-message prepend that re-bills every later turn. `false`
/// when the transcript cannot take the pair (not wire-safe, or a push
/// failed — a half-seated pair is popped so no orphan `tool_use` stays).
pub(crate) fn seat_evidence(session: &mut Session, evidence: &str) -> bool {
    if !evidence_seating_is_wire_safe(session) {
        return false;
    }
    let id = format!("auto_fanout_{}", session.messages.len());
    let tool_use = ConversationMessage::assistant(vec![ContentBlock::ToolUse {
        id: id.clone(),
        name: PRELUDE_TOOL.to_string(),
        input: r#"{"reason":"parallel pre-analysis"}"#.to_string(),
    }]);
    if session.push_message(tool_use).is_err() {
        return false;
    }
    let tool_result = ConversationMessage::tool_result(id, PRELUDE_TOOL, evidence, false);
    if session.push_message(tool_result).is_err() {
        Arc::make_mut(&mut session.messages).pop();
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::{evidence_seating_is_wire_safe, inline_input, prelude_banner, seat_evidence};
    use runtime::{ContentBlock, ConversationMessage, MessageRole, Session};

    fn session_with(messages: Vec<ConversationMessage>) -> Session {
        let mut session = Session::new();
        for message in messages {
            session.push_message(message).expect("push");
        }
        session
    }

    #[test]
    fn followup_decomposition_carries_the_project_discussion() {
        let session = session_with(vec![ConversationMessage::user_text("Review /Users/dev/card, the card services")]);
        let context = super::decomposition_context(&session, "이걸 v2로 병렬 분석해");
        assert!(context.contains("/Users/dev/card"));
        assert!(context.ends_with("이걸 v2로 병렬 분석해"));
    }

    #[test]
    fn a_synthetic_assistant_turn_needs_a_user_side_message_before_it() {
        assert!(!evidence_seating_is_wire_safe(&Session::new()));
        assert!(evidence_seating_is_wire_safe(&session_with(vec![
            ConversationMessage::user_text("look at the repo"),
        ])));
        assert!(!evidence_seating_is_wire_safe(&session_with(vec![
            ConversationMessage::user_text("look at the repo"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "done".to_string(),
            }]),
        ])));
    }

    #[test]
    fn seating_appends_a_matched_tool_pair_or_nothing() {
        let mut session = session_with(vec![ConversationMessage::user_text("first")]);
        assert!(seat_evidence(&mut session, "### alpha\nfinding"));
        let roles: Vec<MessageRole> = session
            .messages
            .iter()
            .map(|message| message.role)
            .collect();
        assert_eq!(
            roles,
            vec![MessageRole::User, MessageRole::Assistant, MessageRole::Tool]
        );
        let (use_id, result_id) = match (&session.messages[1].blocks[0], &session.messages[2].blocks[0]) {
            (
                ContentBlock::ToolUse { id, name, .. },
                ContentBlock::ToolResult {
                    tool_use_id,
                    tool_name,
                    output,
                    ..
                },
            ) => {
                assert_eq!((name.as_str(), tool_name.as_str()), ("SpawnMultiAgent", "SpawnMultiAgent"));
                assert!(output.contains("finding"));
                (id.clone(), tool_use_id.clone())
            }
            other => panic!("not a tool pair: {other:?}"),
        };
        assert_eq!(use_id, result_id);

        // Not wire-safe: the transcript ends on an assistant message.
        let mut ended = session_with(vec![
            ConversationMessage::user_text("first"),
            ConversationMessage::assistant(vec![ContentBlock::Text {
                text: "answer".to_string(),
            }]),
        ]);
        assert!(!seat_evidence(&mut ended, "evidence"));
        assert_eq!(ended.messages.len(), 2, "nothing half-seated");
    }

    #[test]
    fn the_banner_and_the_inline_fallback_carry_the_facts() {
        assert_eq!(
            prelude_banner(4, runtime::RouteTaskComplexity::Large),
            "route · large task · pre-analysis fan-out · up to 4 agents"
        );
        assert_eq!(
            inline_input("migrate it", "[Smart pre-analysis]\nx"),
            "migrate it\n\n---\n[Smart pre-analysis]\nx"
        );
    }
}
