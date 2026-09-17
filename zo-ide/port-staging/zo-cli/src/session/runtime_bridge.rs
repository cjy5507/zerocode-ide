//! Live `AsyncApiClient` adapter wiring `ConversationRuntime::run_turn_streaming`
//! to the production provider HTTP/SSE transport.
//!
//! This module is the L7c-C bridge: it owns the cloned `ProviderClient`
//! plus the per-turn parameters needed to lower a runtime
//! [`ApiRequest`](runtime::ApiRequest) into the provider-neutral
//! [`api::MessageRequest`] wire shape, then drives a fresh
//! `HttpSource` through `parse_source_with_events` so a single SSE pass
//! feeds both the TUI render channel **and** the runtime's
//! `AssistantEvent` bookkeeping in one shot.
//!
//! ## Living standard
//!
//! 1. Module layout: one file, one concern (no submodules).
//! 2. Errors: one `thiserror`-derived enum
//!    ([`RuntimeBridgeError`]); no `anyhow`.
//! 3. Async trait impl is hand-rolled
//!    `Pin<Box<dyn Future + Send + 'a>>` per the L1 living standard.
//! 4. Tests live at
//!    `crates/zo-cli/tests/session_integration.rs`.
//! 5. Every `pub` item carries a `///` doc comment.
//!
//! Code-rule references: R1 (provider neutrality at the trait
//! boundary), R6 (typed errors), R8 (bounded channels — `render_tx` is
//! supplied by the caller).
//!
//! [`LiveAsyncApiClient`] is the live async bridge used by the streaming
//! TTY turn loop (`turn_controller::drive_turn`) and the ndjson sink.

use std::pin::Pin;

use api::{AuthRoute, ApiError, MessageRequest, ProviderClient, ProviderErrorClass, ToolChoice};
use runtime::message_stream::StreamError;
use runtime::message_stream::anthropic::AnthropicStream;
use runtime::message_stream::anthropic::source::HttpSource;
use runtime::message_stream::{BlockId, BlockIdGen, RenderBlock, SystemLevel};
use runtime::{ApiRequest, AssistantEvent, AsyncApiClient, RuntimeError};
use std::sync::Arc;
use std::sync::atomic::AtomicU64;
use tokio::sync::mpsc;
use tools::GlobalToolRegistry;

use crate::cli_args::AllowedToolSet;
use crate::{
    convert_messages, filter_tool_specs, mark_conversation_cache_breakpoints, max_tokens_for_model,
};

/// Errors surfaced by the live streaming bridge.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeBridgeError {
    /// Underlying transport failure surfaced from the `api` crate.
    #[error("provider transport: {message}")]
    Transport {
        message: String,
        provider_error_class: Option<ProviderErrorClass>,
    },

    /// SSE parser surfaced an error while draining the stream.
    #[error("provider stream: {message}")]
    Stream {
        message: String,
        provider_error_class: Option<ProviderErrorClass>,
    },
}

impl RuntimeBridgeError {
    fn transport(message: impl Into<String>) -> Self {
        Self::Transport {
            message: message.into(),
            provider_error_class: None,
        }
    }

    fn from_api_error(error: &ApiError) -> Self {
        Self::Transport {
            message: error.to_string(),
            provider_error_class: Some(error.provider_error_class()),
        }
    }

    fn from_stream_error(error: &StreamError) -> Self {
        Self::Stream {
            message: error.to_string(),
            provider_error_class: error.provider_error_class(),
        }
    }

    fn provider_error_class(&self) -> Option<ProviderErrorClass> {
        match self {
            Self::Transport {
                provider_error_class,
                ..
            }
            | Self::Stream {
                provider_error_class,
                ..
            } => *provider_error_class,
        }
    }
}

impl From<RuntimeBridgeError> for RuntimeError {
    fn from(error: RuntimeBridgeError) -> Self {
        if let Some(provider_error_class) = error.provider_error_class() {
            RuntimeError::with_provider_error_class(error.to_string(), provider_error_class)
        } else {
            RuntimeError::new(error.to_string())
        }
    }
}

/// Build the wire-level [`MessageRequest`] for one runtime turn.
///
/// Mirrors the legacy synchronous path in
/// `AnthropicRuntimeClient::stream` exactly so byte-for-byte parity
/// with the non-TTY ndjson harness is preserved.
#[must_use]
#[allow(clippy::too_many_arguments)]
pub fn build_message_request(
    request: &ApiRequest,
    model: &str,
    enable_tools: bool,
    allowed_tools: Option<&AllowedToolSet>,
    tool_registry: &GlobalToolRegistry,
    thinking: Option<api::ThinkingConfig>,
    named_effort: Option<api::EffortLevel>,
    effort_band_ceiling: Option<api::EffortLevel>,
) -> MessageRequest {
    // Per-turn wire-model override (refusal → Opus 4.8 fallback). Only the wire
    // model id and its `max_tokens` change; the bound Anthropic client is
    // unchanged (the fallback target is Anthropic, same as the refused Fable).
    let selected_model = request.model_override.as_deref().unwrap_or(model);
    let wire_model = api::wire_model_id(selected_model);
    // The advertised set must not depend on which leg of a turn this is: tool
    // definitions sit in front of the messages in the cached prefix, so a set
    // that changes between two requests of one conversation re-bills the entire
    // history behind it. A bounded PLAN/VERIFY leg still may not delegate — that
    // rule is enforced at authorize time by `DEEP_LEG_DELEGATION_TOOLS`, which
    // costs a refused call instead of a cold prefix.
    let tools = enable_tools.then(|| filter_tool_specs(tool_registry, allowed_tools));
    // Reconcile history against the tools we are actually advertising: a stored
    // tool_use naming a tool no longer in this set (e.g. an MCP server dropped or
    // allowed_tools narrowed) would hard-400 the OpenAI-compatible path.
    let known = known_tool_names(tools.as_deref());
    let reconciled = runtime::session::reconcile_tool_history(&request.messages, &known);
    let mut messages = convert_messages(&reconciled);
    // Reminders ride the newest user message so the system blocks — and the
    // cached history behind them — stay byte-identical across turns. Before
    // the breakpoints: the reminder tail is what the last-message breakpoint
    // should cover.
    runtime::append_wire_reminders(&mut messages, &request.wire_reminders);
    mark_conversation_cache_breakpoints(&mut messages);

    // Provider-neutral effort: derive it from the selected thinking budget so
    // both wire shapes are driven from one control. Adaptive Anthropic models
    // (Opus 4.6+/Fable) turn this into `output_config.effort` at the wire seam
    // (`normalize_thinking_for_wire`); GPT backends read it as `reasoning_effort`;
    // legacy Anthropic models keep using `thinking.budget_tokens`. A zero/absent
    // budget means "no explicit effort" — the backend default applies.
    //
    // `effort_override` is a floor (deep-gate escalation): it can only raise the
    // budget, never lower it. Rebuild `thinking` from the floored budget so a
    // legacy model's `budget_tokens` is escalated too, not just the adaptive
    // effort level.
    let configured_budget = thinking
        .as_ref()
        .and_then(|t| t.budget_tokens)
        .filter(|&budget| budget > 0);
    let effective_budget =
        api::effort_budget_with_floor(configured_budget, request.effort_override);
    let thinking = effective_budget.map_or(thinking, |b| Some(api::ThinkingConfig::enabled(b)));
    // Preserve a named preset independently of its legacy numeric budget while
    // still allowing a deep-gate floor to raise lower named tiers. The merge is
    // a tier maximum, so a floor can never lower explicit Ultra — EXCEPT when
    // `effort_band_ceiling` is Some (Smart mode): the merge is bypassed
    // entirely, or Smart's ever-present 28k legacy budget would derive a Max
    // floor (rank 4) that outranks the intended Xhigh band floor (rank 3),
    // silently re-pinning static Max and destroying the whole point of the
    // dynamic band. The thinking BUDGET NUMBER above still incorporates the
    // deep-gate floor either way; only this named-tier merge is bypassed.
    let effort = effort_with_budget_floor(named_effort, effective_budget, effort_band_ceiling);

    MessageRequest {
        model: wire_model.clone(),
        max_tokens: max_tokens_for_model(&wire_model),
        messages,
        system: (!request.system_prompt.is_empty()).then(|| {
            let joined = request.system_prompt.join("\n\n");
            split_system_with_identity(&joined)
        }),
        tools,
        tool_choice: enable_tools.then_some(ToolChoice::Auto),
        stream: true,
        thinking,
        output_config: None,
        effort,
        effort_band_ceiling,
    }
}

/// Merge an explicit named tier with a budget-derived escalation floor.
/// Named Ultra remains the top tier; lower named tiers may be raised.
///
/// `effort_band_ceiling: Some(_)` (Smart mode) bypasses the merge entirely
/// and returns `named` untouched: `named` is the band's FLOOR (Xhigh), and
/// the budget-derived floor must never displace it — see the call site doc
/// in [`build_message_request`] for why.
pub(crate) fn effort_with_budget_floor(
    named: Option<api::EffortLevel>,
    effective_budget: Option<u32>,
    effort_band_ceiling: Option<api::EffortLevel>,
) -> Option<api::EffortLevel> {
    if effort_band_ceiling.is_some() {
        return named;
    }
    let budget_level = effective_budget.map(api::effort_level_for_budget);
    match (named, budget_level) {
        (Some(named), Some(budget)) if api::effort_rank(budget) > api::effort_rank(named) => {
            Some(budget)
        }
        (Some(named), _) => Some(named),
        (None, budget) => budget,
    }
}

/// Names of the tools being advertised, for tool-history reconciliation. `None`
/// (tools disabled) yields an empty set, so any historical `tool_use` is
/// rewritten to text — a request with no tool list cannot carry a `tool_use`.
fn known_tool_names(tools: Option<&[api::ToolDefinition]>) -> std::collections::BTreeSet<String> {
    tools
        .into_iter()
        .flatten()
        .map(|def| def.name.clone())
        .collect()
}

/// Lower the runtime system prompt into wire-level [`api::SystemBlock`]s.
///
/// Thin re-export of the shared [`runtime::split_system_with_identity`] — the
/// single source of truth for identity isolation + cache breakpoints, now also
/// used by the sub-agent provider client so background agents send the exact
/// same system shape as the foreground turn (CC 429-parity).
pub fn split_system_with_identity(system_text: &str) -> Vec<api::SystemBlock> {
    runtime::split_system_with_identity(system_text)
}

/// `AsyncApiClient` implementation backed by a live
/// [`ProviderClient`].
///
/// One instance is constructed per streaming TTY turn (see
/// `turn_controller::drive_turn`). It owns a cloned `ProviderClient` plus the
/// per-turn parameters required to rebuild the wire request, and
/// implements [`AsyncApiClient::stream_async`] by:
///
/// 1. Lowering the runtime [`ApiRequest`] into a `MessageRequest`.
/// 2. Calling `client.stream_message(..)` to obtain a live
///    `api::MessageStream`.
/// 3. Wrapping the stream in [`HttpSource`] and feeding it through
///    [`AnthropicStream::parse_source_with_events`] so the same SSE
///    pass populates both the TUI `render_tx` channel and the
///    `AssistantEvent` bookkeeping the runtime needs.
pub struct LiveAsyncApiClient {
    client: ProviderClient,
    model: String,
    auth_route: AuthRoute,
    enable_tools: bool,
    allowed_tools: Option<AllowedToolSet>,
    tool_registry: GlobalToolRegistry,
    thinking: Option<api::ThinkingConfig>,
    named_effort: Option<api::EffortLevel>,
    /// `Some(ceiling)` when `named_effort` is Smart's dynamic-band floor
    /// (Xhigh) rather than a static pin — threaded straight through to
    /// [`build_message_request`] every request this client builds.
    effort_band_ceiling: Option<api::EffortLevel>,
}

impl LiveAsyncApiClient {
    /// Build a fresh live client. The arguments mirror the subset of
    /// `AnthropicRuntimeClient` state required to lower an
    /// [`ApiRequest`] into the wire shape.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        client: ProviderClient,
        model: String,
        auth_route: AuthRoute,
        enable_tools: bool,
        allowed_tools: Option<AllowedToolSet>,
        tool_registry: GlobalToolRegistry,
        thinking: Option<api::ThinkingConfig>,
        named_effort: Option<api::EffortLevel>,
        effort_band_ceiling: Option<api::EffortLevel>,
    ) -> Self {
        Self {
            client,
            model,
            auth_route,
            enable_tools,
            allowed_tools,
            tool_registry,
            thinking,
            named_effort,
            effort_band_ceiling,
        }
    }
}

impl AsyncApiClient for LiveAsyncApiClient {
    #[allow(clippy::too_many_lines)]
    fn stream_async<'a>(
        &'a self,
        request: ApiRequest,
        render_tx: mpsc::Sender<RenderBlock>,
        text_block_id: BlockId,
    ) -> Pin<
        Box<
            dyn std::future::Future<Output = Result<Vec<AssistantEvent>, RuntimeError>> + Send + 'a,
        >,
    > {
        Box::pin(async move {
            let __wire_t = std::time::Instant::now();
            // Build the wire request off the async executor thread. `convert_messages`
            // re-compresses every historical tool result (uncached) and clones the
            // whole conversation into the provider payload; right after an auto
            // fan-out injects three agents' (often large) outputs into context, that
            // is seconds of pure CPU. Run inline it holds `drive_turn`'s `select!`
            // without yielding, so the spinner *and* input freeze until it returns
            // (the reported ~5s "완전히 얼어붙음" on the first tool call). Hopping to
            // the blocking pool keeps `render_tick` painting while it runs; the
            // owned `request` moves in and every other input is a cheap clone
            // (`tool_registry` is Arc-backed by design).
            let wire = {
                let model = self.model.clone();
                let enable_tools = self.enable_tools;
                let allowed_tools = self.allowed_tools.clone();
                let tool_registry = self.tool_registry.clone();
                let thinking = self.thinking.clone();
                let named_effort = self.named_effort;
                let effort_band_ceiling = self.effort_band_ceiling;
                tokio::task::spawn_blocking(move || {
                    build_message_request(
                        &request,
                        &model,
                        enable_tools,
                        allowed_tools.as_ref(),
                        &tool_registry,
                        thinking,
                        named_effort,
                        effort_band_ceiling,
                    )
                })
                .await
                .map_err(|err| {
                    RuntimeBridgeError::transport(format!("request build task failed: {err}"))
                })?
            };
            if __wire_t.elapsed().as_millis() >= 50 && runtime::turn_profiling_enabled() {
                eprintln!(
                    "[TURN-SEG] build_message_request = {}ms (offloaded to spawn_blocking; render_tick stays live)",
                    __wire_t.elapsed().as_millis()
                );
            }

            // The shared id generator starts at the runtime's pre-allocated
            // text block id so retry notices and this iteration's streamed
            // blocks sit on one contiguous numbering range.
            let ids = BlockIdGen(Arc::new(AtomicU64::new(text_block_id.0)));
            let retry_ids = ids.clone();
            let retry_tx = render_tx.clone();
            let client = self
                .client
                .clone()
                .with_anthropic_retry_notice_callback(move |notice| {
                    // One classifier for the label, the backoff schedule, and the
                    // quota-escape decision: a row that says "rate limited" while
                    // the retry layer is treating the same error as a provider
                    // overload teaches the user to distrust both.
                    let label = core_types::retry_signal::retry_notice_label(&notice.error);
                    // Surface the underlying cause: a bare "retrying" row left
                    // users unable to tell a 429 from an overload or a network
                    // drop during multi-attempt ladders.
                    let mut reason: String = notice
                        .error
                        .chars()
                        .map(|ch| if ch == '\n' || ch == '\r' { ' ' } else { ch })
                        .take(120)
                        .collect();
                    if notice.error.chars().count() > 120 {
                        reason.push('…');
                    }
                    let text = format!(
                        "{label}; retrying in {:.0}s (attempt {}/{}) — {reason}",
                        notice.delay.as_secs_f64().ceil(),
                        notice.attempt,
                        notice.max_attempts
                    );
                    let _ = retry_tx.try_send(RenderBlock::System {
                        id: retry_ids.next(),
                        level: SystemLevel::Warn,
                        text,
                    });
                });

            // Establish the upstream stream. A cached OAuth bearer that lapsed
            // mid-turn surfaces as a server 401 here (the request path uses a
            // bare snapshot and never refreshes per-request), so refresh once
            // and retry instead of failing the turn until the process restarts.
            // The 401 is raised at establishment, before any block is rendered,
            // so the retry never double-renders. Recovery is per-provider: the
            // Anthropic client swaps its bearer in place, while OAuth-backed
            // non-Anthropic clients (Gemini Code Assist / ChatGPT) capture their
            // bearer at construction and only rotate by rebuilding — a plain
            // `with_anthropic_auth` swap is a no-op for them and would retry with
            // the identical stale token and 401 again.
            let stream = match client.stream_message(&wire).await {
                Ok(stream) => stream,
                Err(err) if err.is_unauthorized() => {
                    match recover_oauth_client_after_401(&client, self.auth_route).await {
                        Some(recovered) => recovered
                            .stream_message(&wire)
                            .await
                            .map_err(|err| RuntimeBridgeError::from_api_error(&err))?,
                        None => return Err(RuntimeBridgeError::from_api_error(&err).into()),
                    }
                }
                Err(err) => return Err(RuntimeBridgeError::from_api_error(&err).into()),
            };

            // Surface the streaming backends' internal mid-stream restarts (a
            // pre-commit stall re-opens the upstream connection transparently and
            // never returns an error the establish-time retry layer above could
            // render, so the turn would otherwise just freeze for the backoff).
            // Covers ChatGPT, OpenAI-compatible, and Gemini Code Assist streams;
            // the Anthropic stream's restart is deliberately silent. Mirrors the
            // establish-time Anthropic notice wording.
            let restart_ids = ids.clone();
            let restart_tx = render_tx.clone();
            let stream = stream.with_stream_retry_notice(move |notice| {
                let (level, text) = stream_notice_row(&notice);
                let _ = restart_tx.try_send(RenderBlock::System {
                    id: restart_ids.next(),
                    level,
                    text,
                });
            });

            let source = HttpSource::new(stream);

            let outputs = AnthropicStream::parse_source_with_events(source, render_tx, ids)
                .await
                .map_err(|err| RuntimeBridgeError::from_stream_error(&err))?;
            Ok(outputs.events)
        })
    }
}

/// One transcript row for a mid-stream notice. Reconnects render as warnings
/// with the backoff/attempt detail; the quiet-reasoning heartbeat renders as
/// an informational row (nothing is wrong — the model is thinking silently on
/// a connection that is verifiably alive), so minutes of "no output" stop
/// reading as a hang. Pure so the wording is unit-testable.
fn stream_notice_row(notice: &core_types::StreamRetryNotice) -> (SystemLevel, String) {
    match notice.kind {
        core_types::StreamNoticeKind::QuietReasoning => (
            SystemLevel::Info,
            format!(
                "{} ({}s+ without visible output)",
                notice.label,
                notice.delay.as_secs()
            ),
        ),
        core_types::StreamNoticeKind::Reconnect => (
            SystemLevel::Warn,
            format!(
                "{}; reconnecting in {:.0}s (attempt {}/{})",
                notice.label,
                notice.delay.as_secs_f64().ceil(),
                notice.attempt,
                notice.max_attempts
            ),
        ),
    }
}

/// Recover a fresh provider client after a mid-turn 401, dispatching per
/// provider. Mirrors the pre-turn refresh in
/// [`crate::runtime_support::refresh_oauth_if_near_expiry`]: the Anthropic
/// client swaps its bearer in place from the re-resolved OAuth chain, while
/// OAuth-backed non-Anthropic clients (Gemini Code Assist / ChatGPT) capture
/// their bearer at construction and only rotate by rebuilding through the
/// provider's own loader (`from_model_with_anthropic_auth` re-runs
/// `load_fresh_oauth` / `load_fresh_openai_oauth`, each of which refreshes and
/// re-persists a near-expiry token). For those a plain `with_anthropic_auth`
/// swap is a no-op, so the retry would reuse the identical stale bearer and 401
/// again. The rebuild runs the provider loader, which may perform a network
/// OAuth round-trip, so it hops to a blocking thread. Returns the client to
/// retry the request with, or `None` when no recovery is possible.
async fn recover_oauth_client_after_401(
    client: &ProviderClient,
    auth_route: AuthRoute,
) -> Option<ProviderClient> {
    if matches!(
        client,
        ProviderClient::GeminiCodeAssist(_) | ProviderClient::ChatGpt(_)
    ) {
        let provider_kind = client.provider_kind();
        // Carry the session-pinned prompt-cache scope onto the fresh client:
        // the rebuild replaces the whole instance, and losing the scope here
        // would roll the provider cache key on every 401 recovery.
        let cache_scope = client.pinned_cache_scope().map(str::to_string);
        return tokio::task::spawn_blocking(move || {
            let rebuilt = ProviderClient::from_provider_kind_with_auth_route(
                provider_kind,
                auth_route,
            )
            .ok()?;
            Some(match cache_scope {
                Some(scope) => rebuilt.with_cache_scope(&scope),
                None => rebuilt,
            })
        })
        .await
        .ok()
        .flatten();
    }

    if auth_route == AuthRoute::ApiKey {
        return None;
    }
    let fresh = crate::runtime_support::refresh_claude_oauth().await?;
    Some(client.clone().with_anthropic_auth(fresh))
}

#[cfg(test)]
mod tests {
    use super::*;

    use runtime::CLAUDE_CODE_IDENTITY;

    fn block_text(block: &api::SystemBlock) -> &str {
        let api::SystemBlock::Text { text, .. } = block;
        text
    }

    fn block_cache(block: &api::SystemBlock) -> Option<&api::CacheControl> {
        let api::SystemBlock::Text { cache_control, .. } = block;
        cache_control.as_ref()
    }

    fn advertised_tool_names(
        registry: &GlobalToolRegistry,
        model: &str,
    ) -> std::collections::BTreeSet<String> {
        advertised_tool_names_for_messages(registry, model, Vec::new())
    }

    fn advertised_tool_names_for_messages(
        registry: &GlobalToolRegistry,
        model: &str,
        messages: Vec<runtime::ConversationMessage>,
    ) -> std::collections::BTreeSet<String> {
        let request = ApiRequest {
            system_prompt: Arc::from(Vec::<String>::new()),
            wire_reminders: Arc::from(Vec::<String>::new()),
            messages: Arc::new(messages),
            tool_choice: None,
            effort_override: None,
            model_override: None,
        };
        build_message_request(&request, model, true, None, registry, None, None, None)
            .tools
            .unwrap_or_default()
            .into_iter()
            .map(|tool| tool.name)
            .collect()
    }

    /// The advertised tool set must not move when a deep leg runs.
    ///
    /// It used to: a PLAN or VERIFY leg dropped the four delegation tools from
    /// the wire, and the next ordinary turn put them back. Tool definitions sit
    /// in front of the messages in the cached prefix, so each flip re-billed the
    /// whole conversation behind them — measured at 41% of one day's cache-write,
    /// every occurrence a fully cold read. The leg still may not delegate; that
    /// is now `DEEP_LEG_DELEGATION_TOOLS` denying the call at authorize time
    /// (see `deep_leg_phase_blocks_delegation_tools_without_touching_the_wire`
    /// in the runtime crate), which costs one refused call instead.
    #[test]
    fn a_deep_leg_prompt_does_not_change_the_advertised_tool_set() {
        let registry = GlobalToolRegistry::builtin();
        let delegation = ["Agent", "SpawnMultiAgent", "Workflow", "SendMessage"];

        // Activate them first. All four are deferred, so the claim is only
        // observable once they would otherwise be on the wire.
        let activated = registry.search(
            "select:Agent,SpawnMultiAgent,Workflow,SendMessage",
            8,
            None,
            None,
        );
        assert_eq!(activated.matches.len(), delegation.len());

        let ordinary = advertised_tool_names_for_messages(
            &registry,
            "gpt-5.6-sol",
            vec![runtime::ConversationMessage::user_text("implement the next change")],
        );
        for name in delegation {
            assert!(ordinary.contains(name), "premise: {name} is on the wire");
        }

        for marker in ["[deep:PLAN] inspect and plan", "[deep:VERIFY] judge the diff"] {
            let leg = advertised_tool_names_for_messages(
                &registry,
                "gpt-5.6-sol",
                vec![runtime::ConversationMessage::user_text(marker)],
            );
            assert_eq!(
                leg, ordinary,
                "{marker} changed the advertised set — that strands the whole prefix"
            );
        }
    }

    /// OpenAI defers like every other provider.
    ///
    /// It used to front-load the whole deferred set so that a `ToolSearch`
    /// activation could not mutate the cached tool prefix. That put ~55 KB of
    /// schema on every request, including turns that call no tool at all.
    #[test]
    fn openai_requests_defer_builtins_until_activation() {
        let registry = GlobalToolRegistry::builtin();
        let before = advertised_tool_names(&registry, "gpt-5.6-sol");

        for name in ["WebFetch", "Workflow", "TaskList", "REPL"] {
            assert!(!before.contains(name), "OpenAI request must defer {name}");
        }

        let output = registry.search("select:Workflow", 3, None, None);
        assert_eq!(output.matches, vec!["Workflow".to_string()]);
        let after = advertised_tool_names(&registry, "gpt-5.6-sol");
        assert!(after.contains("Workflow"), "activation advertises the tool");
        assert!(!after.contains("TaskList"), "unactivated tools stay deferred");
    }

    #[test]
    fn anthropic_requests_keep_deferred_builtins_until_activation() {
        let registry = GlobalToolRegistry::builtin();
        let before = advertised_tool_names(&registry, "claude-sonnet-4-6");
        assert!(!before.contains("Workflow"));

        let output = registry.search("select:Workflow", 3, None, None);
        assert_eq!(output.matches, vec!["Workflow".to_string()]);
        let after = advertised_tool_names(&registry, "claude-sonnet-4-6");
        assert!(after.contains("Workflow"));
        assert!(!after.contains("TaskList"), "unactivated tools stay deferred");
    }

    /// Advertisement no longer varies by provider: the deferred set is the same
    /// wherever the turn is routed, so swapping models mid-session cannot
    /// change the tool surface underneath the model.
    #[test]
    fn builtin_advertisement_is_identical_across_a_model_swap() {
        let registry = GlobalToolRegistry::builtin();
        let openai = advertised_tool_names(&registry, "gpt-5.6-sol");
        let anthropic = advertised_tool_names(&registry, "claude-sonnet-4-6");

        assert_eq!(openai, anthropic);
        for name in ["WebFetch", "Workflow", "TaskList", "REPL"] {
            assert!(!openai.contains(name), "{name} stays deferred on both");
        }
    }

    /// The quiet-reasoning heartbeat must read as information, never as a
    /// reconnect warning — its whole point is "nothing is wrong".
    #[test]
    fn quiet_reasoning_notice_renders_as_info_not_reconnect() {
        let quiet = core_types::StreamRetryNotice {
            kind: core_types::StreamNoticeKind::QuietReasoning,
            label: "model reasoning silently — stream alive",
            attempt: 0,
            max_attempts: 0,
            delay: std::time::Duration::from_secs(61),
        };
        let (level, text) = stream_notice_row(&quiet);
        assert_eq!(level, SystemLevel::Info);
        assert!(text.contains("stream alive"), "text: {text}");
        assert!(text.contains("61s+"), "text: {text}");
        assert!(!text.contains("reconnecting"), "text: {text}");

        let reconnect = core_types::StreamRetryNotice {
            kind: core_types::StreamNoticeKind::Reconnect,
            label: "connection dropped",
            attempt: 2,
            max_attempts: 3,
            delay: std::time::Duration::from_secs(4),
        };
        let (level, text) = stream_notice_row(&reconnect);
        assert_eq!(level, SystemLevel::Warn);
        assert!(text.contains("reconnecting in 4s (attempt 2/3)"), "text: {text}");
    }

    #[test]
    fn build_message_request_preserves_named_ultra_alongside_20k_budget() {
        let request = ApiRequest {
            system_prompt: Arc::from(Vec::<String>::new()),
            wire_reminders: Arc::from(Vec::<String>::new()),
            messages: Arc::new(Vec::new()),
            tool_choice: None,
            effort_override: None,
            model_override: None,
        };
        let wire = build_message_request(
            &request,
            "gpt-5.6-sol",
            false,
            None,
            &GlobalToolRegistry::builtin(),
            Some(api::ThinkingConfig::enabled(20_000)),
            Some(api::EffortLevel::Ultra),
            None,
        );
        assert_eq!(wire.thinking.unwrap().budget_tokens, Some(20_000));
        assert_eq!(wire.effort, Some(api::EffortLevel::Ultra));
        assert_eq!(wire.effort_band_ceiling, None);

        assert_eq!(
            effort_with_budget_floor(Some(api::EffortLevel::Low), Some(16_000), None),
            Some(api::EffortLevel::Xhigh),
            "a deep-gate floor must still raise lower named tiers"
        );
        assert_eq!(
            effort_with_budget_floor(Some(api::EffortLevel::Ultra), Some(24_000), None),
            Some(api::EffortLevel::Ultra),
            "a budget floor must never lower explicit Ultra"
        );
    }

    #[test]
    fn build_message_request_strips_provider_qualification_from_wire_model() {
        let request = ApiRequest {
            system_prompt: Arc::from(Vec::<String>::new()),
            wire_reminders: Arc::from(Vec::<String>::new()),
            messages: Arc::new(Vec::new()),
            tool_choice: None,
            effort_override: None,
            model_override: None,
        };
        let wire = build_message_request(
            &request,
            "google/gemini-3.6-flash",
            false,
            None,
            &GlobalToolRegistry::builtin(),
            None,
            None,
            None,
        );
        assert_eq!(wire.model, "gemini-3.6-flash");
    }

    #[test]
    fn build_message_request_bypasses_the_budget_floor_for_a_banded_smart_request() {
        // The seam bug this defuses: Smart's ever-present 28k legacy budget
        // resolves via `effort_level_for_budget` to Max (rank 4), which would
        // outrank the intended Xhigh band floor (rank 3) in the normal merge
        // and silently re-pin static Max — destroying the whole point of the
        // dynamic band. `effort_band_ceiling: Some(_)` must bypass that merge.
        let request = ApiRequest {
            system_prompt: Arc::from(Vec::<String>::new()),
            wire_reminders: Arc::from(Vec::<String>::new()),
            messages: Arc::new(Vec::new()),
            tool_choice: None,
            effort_override: None,
            model_override: None,
        };
        let wire = build_message_request(
            &request,
            "gpt-5.6-sol",
            false,
            None,
            &GlobalToolRegistry::builtin(),
            // Smart's legacy 28k budget — would derive Max via
            // `effort_level_for_budget` if the merge were not bypassed.
            Some(api::ThinkingConfig::enabled(28_000)),
            Some(api::EffortLevel::Xhigh),
            Some(api::EffortLevel::Max),
        );
        assert_eq!(
            wire.effort,
            Some(api::EffortLevel::Xhigh),
            "banded floor must survive the 28k budget untouched, not get re-pinned to Max"
        );
        assert_eq!(wire.effort_band_ceiling, Some(api::EffortLevel::Max));
        // The thinking budget NUMBER itself is unaffected by the bypass — it
        // still rides for Anthropic's adaptive-thinking token accounting.
        assert_eq!(wire.thinking.unwrap().budget_tokens, Some(28_000));

        assert_eq!(
            effort_with_budget_floor(
                Some(api::EffortLevel::Xhigh),
                Some(24_000),
                Some(api::EffortLevel::Ultra)
            ),
            Some(api::EffortLevel::Xhigh),
            "Some(ceiling) bypasses the merge entirely, regardless of the budget"
        );
    }

    #[test]
    fn splits_identity_static_and_dynamic_dropping_the_marker() {
        let prompt = format!(
            "{CLAUDE_CODE_IDENTITY}\n\n# Static guidance\nrules\n\n{}\n\n# Project context\ngit dirty",
            runtime::SYSTEM_PROMPT_DYNAMIC_BOUNDARY
        );
        let blocks = split_system_with_identity(&prompt);

        assert_eq!(blocks.len(), 3, "identity + static + dynamic");
        // Identity must be the verbatim first block with no cache_control
        // (Claude Max OAuth fingerprint requirement).
        assert_eq!(block_text(&blocks[0]), CLAUDE_CODE_IDENTITY);
        assert!(block_cache(&blocks[0]).is_none());
        // Static scaffolding caches independently of the dynamic tail.
        assert!(block_text(&blocks[1]).contains("# Static guidance"));
        assert_eq!(
            block_cache(&blocks[1]),
            Some(&api::CacheControl::ephemeral_1h())
        );
        // Dynamic context after the boundary.
        assert!(block_text(&blocks[2]).contains("# Project context"));
        assert_eq!(
            block_cache(&blocks[2]),
            Some(&api::CacheControl::ephemeral_1h())
        );
        // The marker itself never reaches the model.
        for block in &blocks {
            assert!(
                !block_text(block).contains(runtime::SYSTEM_PROMPT_DYNAMIC_BOUNDARY),
                "boundary marker must be stripped from every block"
            );
        }
    }

    #[test]
    fn collapses_to_identity_plus_static_when_dynamic_is_empty() {
        let prompt = format!(
            "{CLAUDE_CODE_IDENTITY}\n\n# Static guidance\nrules\n\n{}",
            runtime::SYSTEM_PROMPT_DYNAMIC_BOUNDARY
        );
        let blocks = split_system_with_identity(&prompt);
        assert_eq!(
            blocks.len(),
            2,
            "empty dynamic tail produces no trailing block"
        );
        assert_eq!(block_text(&blocks[0]), CLAUDE_CODE_IDENTITY);
        assert!(block_text(&blocks[1]).contains("# Static guidance"));
    }

    #[test]
    fn caches_whole_body_when_no_boundary_present() {
        // A custom system prompt with no identity and no boundary marker is
        // cached as a single block (back-compat with bare `--system-prompt`).
        let blocks = split_system_with_identity("custom system prompt");
        assert_eq!(blocks.len(), 1);
        assert_eq!(block_text(&blocks[0]), "custom system prompt");
        assert_eq!(
            block_cache(&blocks[0]),
            Some(&api::CacheControl::ephemeral_1h())
        );
    }

    /// Minimal scoped env-var guard for the recovery test (the `runtime_support`
    /// tests keep their own private copy; this one is independent).
    struct EnvVarGuard {
        key: &'static str,
        original: Option<std::ffi::OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: Option<&str>) -> Self {
            let original = std::env::var_os(key);
            match value {
                Some(value) => std::env::set_var(key, value),
                None => std::env::remove_var(key),
            }
            Self { key, original }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            match &self.original {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    fn temp_config_home(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "zo-runtime-bridge-{name}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp config home");
        dir
    }

    /// A mid-turn 401 on an OAuth-backed non-Anthropic provider must rebuild the
    /// client through the provider loader, not take the Anthropic `with_auth`
    /// no-op (which would retry with the identical stale bearer and 401 again).
    /// We seed the Google Code Assist OAuth store with an expired, refresh-less
    /// token so the loader deterministically reuses the stored bearer with no
    /// network, then assert the recovered client reflects that store token —
    /// only the rebuild path can produce it; the no-op would keep the live one.
    #[test]
    fn non_anthropic_401_rebuilds_client_not_anthropic_no_op() {
        let _env_lock = crate::test_env_lock();
        let config_home = temp_config_home("expired-google-oauth-401");
        let _config_home = EnvVarGuard::set(
            "ZO_CONFIG_HOME",
            Some(config_home.to_str().expect("utf8 config home")),
        );
        let _zo_home = EnvVarGuard::set("ZO_HOME", None);
        let _adapter_gate = EnvVarGuard::set(api::NON_CLAUDE_ADAPTERS_ENV, Some("1"));
        let _disable_external = EnvVarGuard::set("ZO_DISABLE_EXTERNAL_CREDENTIALS", None);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_secs();
        api::oauth_store::save_google_code_assist_oauth(&core_types::OAuthTokenSet {
            access_token: "stored-rebuilt-google-token".to_string(),
            // No refresh token: the loader reuses the expired bearer without a
            // network round-trip, making this a deterministic, hermetic test.
            refresh_token: None,
            expires_at: Some(now.saturating_sub(1)),
            scopes: Vec::new(),
        })
        .expect("save expired google oauth");

        let stale = ProviderClient::GeminiCodeAssist(api::GeminiCodeAssistClient::new(
            "live-stale-google-token",
        ));
        let before = format!("{stale:?}");
        assert!(before.contains("live-stale-google-token"));

        let recovered = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
            .block_on(recover_oauth_client_after_401(&stale, AuthRoute::Auto))
            .expect("non-anthropic recovery should rebuild a client");

        let after = format!("{recovered:?}");
        assert!(
            after.contains("stored-rebuilt-google-token"),
            "non-Anthropic 401 must rebuild from the provider loader, not no-op the stale bearer: {after}"
        );
        assert!(
            !after.contains("live-stale-google-token"),
            "the stale bearer must be dropped by the rebuild: {after}"
        );
        std::fs::remove_dir_all(config_home).ok();
    }
}
