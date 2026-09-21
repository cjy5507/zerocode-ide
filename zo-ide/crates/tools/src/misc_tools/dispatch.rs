//! Name → runner dispatch for every misc tool.
//!
//! Each arm of the match deserialises the JSON input into the typed
//! struct, then forwards to the `run_*` wrapper that lives in
//! `super`. The wrappers in turn call the concrete `execute_*`
//! implementation and pretty-print the result.

use runtime::lsp_client::LspRegistry;
use runtime::permission_enforcer::PermissionEnforcer;
use serde_json::{json, Value};

use super::agent_tools::resolve_subagent_type;
use super::smart_router::{
    apply_smart_models_to_spawn_input_with_auto_types,
    smart_parent_model_for_agent_with_auto_type, ROUTE_MODEL_SMUGGLE_KEY,
    ROUTE_REASON_SMUGGLE_KEY,
};
use super::{
    from_value, maybe_enforce_permission_check, run_agent, run_ask_user_question, run_audit,
    run_config, run_council, run_enter_plan_mode, run_exit_plan_mode, run_list_agents,
    run_memory_write, run_push_notification,
    run_monitor, run_notebook_edit, run_remote_trigger, run_retrieve_tool_output,
    run_schedule_wakeup, run_send_message, run_send_to_user, run_session_recall,
    run_skill_distill, run_skill_review, run_sleep, run_spawn_multi_agent,
    run_stop_agent,
    run_structured_output, run_synthetic_output, run_tool_search,
    AgentInput, AskUserQuestionInput, ConfigInput, CouncilInput, EnterPlanModeInput,
    ExitPlanModeInput, MemoryWriteInput, MonitorInput, NotebookEditInput, RemoteTriggerInput,
    ListAgentsInput, PushNotificationInput, RetrieveToolOutputInput, ScheduleWakeupInput, SendMessageInput, SendToUserInput,
    SessionRecallInput,
    SkillDistillInput, SkillInput, SkillLoadInput, SkillReviewInput, SkillSearchInput,
    SleepInput, SpawnMultiAgentInput,
    StopAgentInput,
    StructuredOutputInput, SyntheticOutputInput, ToolContext, ToolError,
    ToolSearchInput, MAIN_CONVERSATION_TARGET,
};

/// The parent session's LSP registry to share with a spawned sub-agent, or
/// `None` when no server is running. Returning `None` for an empty registry
/// keeps the headless / no-LSP case a clean no-op: no empty-registry `Arc` is
/// cloned across the spawn boundary, and the sub-agent's enrich gate
/// (`!ctx.lsp.is_empty()`) stays skipped exactly as today.
fn parent_lsp(ctx: &ToolContext) -> Option<&LspRegistry> {
    (!ctx.lsp.is_empty()).then_some(&ctx.lsp)
}

/// Owning `tool_use` id the runtime dispatcher smuggles into Spawn-family
/// execution input (`spawn_tool_execution_input` in `runtime::conversation`),
/// stamped onto spawned agent manifests so the TUI attributes each agent to
/// the right transcript batch. `None` for direct/headless invocations.
fn smuggled_tool_call_id(input: &Value) -> Option<String> {
    input
        .get("__zo_tool_call_id")
        .and_then(Value::as_str)
        .map(str::to_owned)
}

/// The spawning session's active permission mode: the registry enforcer's
/// live mode where one exists (sub-agent / headless / serve), else the
/// foreground session mode the TUI records on [`ToolContext`]. Spawn-family
/// dispatch stamps this so child agents are clamped to the parent's
/// privilege instead of the historical `DangerFullAccess` default.
fn active_parent_mode(
    ctx: &ToolContext,
    enforcer: Option<&PermissionEnforcer>,
) -> Option<runtime::PermissionMode> {
    enforcer
        .map(PermissionEnforcer::active_mode)
        .or_else(|| ctx.session_permission_mode())
}

fn stamp_inferred_agent_type(input: &mut AgentInput) -> Option<String> {
    if input.subagent_type.is_some() {
        return None;
    }
    let inferred = resolve_subagent_type(None, &input.description, &input.prompt);
    input.subagent_type = Some(inferred.clone());
    Some(inferred)
}

/// Whether an explicit `subagent_type` resolves to a user-defined CUSTOM agent
/// (`.zo/agents/*.md`) — a deliberate, user-authored specialization worth
/// isolating even on the same model. Built-in read-only types (Explore,
/// code-reviewer, …) need no special case: their read-only prompts already fail
/// the guard's write-intent clause. Generic aliases and invented/bogus names
/// carry no custom definition, so a model cannot dodge the guard by naming a
/// made-up `subagent_type`. Derived from the custom-agent SSOT — no hardcoded
/// type table.
fn spawn_uses_custom_agent(explicit: Option<&str>, description: &str, prompt: &str) -> bool {
    let Some(raw) = explicit.map(str::trim).filter(|kind| !kind.is_empty()) else {
        return false;
    };
    let (_resolved, custom) =
        super::agent_tools::resolve_subagent_type_and_custom_agent(Some(raw), description, prompt);
    custom.is_some()
}

/// Even a read-only Small slice stays inline unless the turn carries the
/// person's explicit delegation request. Unknown host policy stays open.
fn small_spawn_is_wasteful(prompt: &str, policy: Option<crate::TurnAgentPolicy>) -> bool {
    policy.is_some_and(|policy| !policy.user_requested_delegation)
        && super::assess_agent_task(prompt).complexity == runtime::RouteTaskComplexity::Small
}

const SMALL_SPAWN_REFUSAL: &str = "Small delegated task: do it inline. Do not retry this spawn; the person did not request delegation.";

fn refuse_small_spawn(prompt: &str, policy: Option<crate::TurnAgentPolicy>) -> Result<(), ToolError> {
    if small_spawn_is_wasteful(prompt, policy) {
        Err(ToolError::Declined(SMALL_SPAWN_REFUSAL.to_string()))
    } else { Ok(()) }
}

/// CC-style implementation-delegation guard. A MODEL-DRIVEN single `Agent`
/// spawn is WASTEFUL — the main loop should implement it inline instead — when
/// every clause below holds. Each early `return false` is an escape that KEEPS
/// the spawn: unknown per-turn policy (fail open), a user turn that itself
/// warrants delegation, a user-defined custom agent, no parent to compare, a
/// genuinely different model, read-only delegated work, or a hard delegated
/// slice. Only a same-model, simple, generic, non-delegation-worthy
/// implementation folds inline. All signals come from the corpus-pinned SSOT
/// classifiers — no hardcoded tables.
fn same_model_impl_spawn_is_wasteful(
    resolved_model: &str,
    parent_model: Option<&str>,
    delegated: crate::AgentTaskAssessment,
    spawn_is_custom_agent: bool,
    policy: Option<crate::TurnAgentPolicy>,
) -> bool {
    // Unknown per-turn policy (background / non-turn dispatch) → fail open.
    let Some(policy) = policy else {
        return false;
    };
    // The user's turn must itself be a simple Solo ask with no delegation
    // value/request; otherwise the turn genuinely warrants sub-agents.
    if !policy.user_turn_is_solo_simple() {
        return false;
    }
    // A user-defined custom agent is a deliberate specialization — keep it.
    if spawn_is_custom_agent {
        return false;
    }
    // No parent model to compare against (non-live/test harness) → fail open.
    let Some(parent) = parent_model.map(str::trim).filter(|p| !p.is_empty()) else {
        return false;
    };
    // A genuinely different effective model is real diversity — keep it.
    if crate::misc_tools::canonicalize_route_model_id(resolved_model)
        != crate::misc_tools::canonicalize_route_model_id(parent)
    {
        return false;
    }
    // Read-only / research delegated work is a valid same-model sub-agent use.
    if !delegated.has_write_intent {
        return false;
    }
    // Only a SIMPLE delegated slice is wasteful to hand to a same-model clone.
    matches!(
        delegated.complexity,
        runtime::RouteTaskComplexity::Trivial | runtime::RouteTaskComplexity::Small
    )
}

/// The successful tool result returned when the guard folds a spawn: a clear
/// `folded_inline` status so the model implements in the main loop and does not
/// masquerade the (non-)spawn as completed work or retry it.
fn inline_fold_result(model: &str) -> String {
    format!(
        "status: folded_inline — no sub-agent was spawned. This is a simple, single \
         implementation task and you are already running `{model}`, so implement it inline in \
         the main loop now (Claude-Code style: the main agent implements; reserve sub-agents for \
         parallel fan-out, read-only search across many files, a genuinely complex/large task, or \
         a different model). Do NOT retry this spawn."
    )
}

fn single_spawn_member_inline_fold_model(
    input: &SpawnMultiAgentInput,
    parent_model: Option<&str>,
    policy: Option<crate::TurnAgentPolicy>,
) -> Option<String> {
    let [member] = input.agents.as_slice() else {
        return None;
    };
    let prompt = member
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let subagent_type = member.get("subagent_type").and_then(Value::as_str);
    let member_model = |key| {
        member
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
            .map(str::to_string)
    };
    let resolved_model = std::env::var(super::agent_tools::AGENT_MODEL_ENV)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .or_else(|| member_model(ROUTE_MODEL_SMUGGLE_KEY))
        .or_else(|| member_model("model"))
        .or_else(|| {
            parent_model
                .map(str::trim)
                .filter(|model| !model.is_empty())
                .map(str::to_string)
        })
        .unwrap_or_default();
    let delegated = super::assess_agent_task(prompt);
    let spawn_is_custom = spawn_uses_custom_agent(subagent_type, "", prompt);

    same_model_impl_spawn_is_wasteful(
        &resolved_model,
        parent_model,
        delegated,
        spawn_is_custom,
        policy,
    )
    .then_some(resolved_model)
}

fn stamp_inferred_spawn_types(input: &mut SpawnMultiAgentInput) -> Vec<Option<String>> {
    input
        .agents
        .iter_mut()
        .map(|agent| {
            let object = agent.as_object_mut()?;
            object.remove(ROUTE_REASON_SMUGGLE_KEY);
            if object.contains_key("subagent_type") || object.contains_key("subagentType") {
                return None;
            }
            let inferred = resolve_subagent_type(
                None,
                object
                    .get("description")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
                object
                    .get("prompt")
                    .and_then(Value::as_str)
                    .unwrap_or_default(),
            );
            object.insert(
                "subagent_type".to_string(),
                Value::String(inferred.clone()),
            );
            Some(inferred)
        })
        .collect()
}

fn annotate_auto_type_reason(reason: Option<String>, auto_type: Option<&str>) -> Option<String> {
    let auto_type = auto_type?;
    Some(reason.map_or_else(
        || format!("type={auto_type} (auto)"),
        |reason| format!("{reason} · type={auto_type} (auto)"),
    ))
}

fn annotate_spawn_auto_type_reasons(
    input: &mut SpawnMultiAgentInput,
    auto_types: &[Option<String>],
) {
    for (agent, auto_type) in input.agents.iter_mut().zip(auto_types) {
        let Some(auto_type) = auto_type.as_deref() else {
            continue;
        };
        let Some(object) = agent.as_object_mut() else {
            continue;
        };
        let existing = object
            .remove(ROUTE_REASON_SMUGGLE_KEY)
            .and_then(|value| value.as_str().map(str::to_string));
        let reason = annotate_auto_type_reason(existing, Some(auto_type))
            .expect("an auto type always produces a route reason");
        object.insert(ROUTE_REASON_SMUGGLE_KEY.to_string(), Value::String(reason));
    }
}

#[allow(clippy::too_many_lines)] // a flat name → runner table, clearer unsplit
pub(crate) fn dispatch(
    ctx: &ToolContext,
    enforcer: Option<&PermissionEnforcer>,
    name: &str,
    input: &Value,
) -> Option<Result<String, ToolError>> {
    match name {
        "Skill" => Some(from_value::<SkillInput>(input).and_then(|input| super::run_skill(input, ctx))),
        "skill_search" => Some(from_value::<SkillSearchInput>(input).and_then(|input| {
            super::run_skill_search(&input, ctx)
        })),
        "skill_load" => Some(
            from_value::<SkillLoadInput>(input).and_then(|input| super::run_skill_load(&input, ctx)),
        ),
        "SkillDistill" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<SkillDistillInput>(input).and_then(|inp| run_skill_distill(&inp))
            }),
        ),
        "SkillReview" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<SkillReviewInput>(input).and_then(|inp| run_skill_review(&inp))
            }),
        ),
        "Agent" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<AgentInput>(input).and_then(|mut inp| {
                    refuse_small_spawn(&inp.prompt, ctx.turn_agent_policy())?;
                    // Named agents: the model-facing boundary is the ONLY place
                    // the CC identifier grammar is enforced, because internal
                    // spawners (workflow phases, fan-out plumbing) legitimately
                    // pass names it rejects. A validated name is carried
                    // verbatim so `SendMessage(to: name)` resolves it.
                    if let Some(raw) = inp.name.as_deref().filter(|raw| !raw.trim().is_empty()) {
                        match super::agent_tools::validate_addressable_agent_name(raw) {
                            Ok(name) => inp.addressable_name = Some(name),
                            Err(message) => return Err(ToolError::InvalidInput(message)),
                        }
                    }
                    // Capture the model's EXPLICIT agent-type choice before
                    // `stamp_inferred_agent_type` fills an inferred one, so the
                    // guard can tell a user-defined custom agent from a bare
                    // "go implement this" spawn.
                    let explicit_subagent_type = inp.subagent_type.clone();
                    let auto_type = stamp_inferred_agent_type(&mut inp);
                    let parent = ctx.spawn_parent_model();
                    // Route unless THIS agent's own model was chosen
                    // explicitly for it (a sub-agent spawned with a concrete
                    // `model` keeps its nested spawns in that context). A
                    // foreground `/model` pin no longer lands here — it binds
                    // the main turn only — so picking Opus as the session
                    // model no longer drags every Explore/verify spawn onto
                    // Opus. Member-level `model` fields still win inside.
                    // A fork (t-2875) runs on the parent's model by contract:
                    // neither routed nor folded — a fork is a deliberate
                    // context-inheriting split, like a custom agent is a
                    // deliberate specialization.
                    let forking =
                        super::agent_tools::is_fork_type(inp.subagent_type.as_deref());
                    let route = !ctx.active_model_pinned() && !forking;
                    if let Some(choice) =
                        route
                            .then(|| {
                                smart_parent_model_for_agent_with_auto_type(
                                    parent.as_deref(),
                                    &inp,
                                    auto_type.as_deref(),
                                )
                            })
                            .flatten()
                    {
                        inp.route_model = choice.model;
                        inp.route_reason = choice.reason;
                        inp.route_fallback_models = choice.fallback_models;
                        inp.route_effort = choice.effort;
                        inp.route_role = Some(choice.decision_meta.role);
                        inp.route_complexity = Some(choice.decision_meta.complexity);
                        inp.route_risk = Some(choice.decision_meta.risk);
                        inp.route_source = Some(choice.decision_meta.route_source);
                        inp.route_probe_confidence = choice.decision_meta.probe_confidence;
                    }
                    // The person's own words are the strongest pin. A model
                    // they named in this turn ("fable 검토 받아") is the model
                    // that runs — over the router's choice and over the call's
                    // `model` alike (2026-09-09: a Gemini parent read Fable as a
                    // persona and asked for the default Opus; zo obeyed the
                    // call). It rides the trusted-route road, so the family
                    // gate does not re-judge what the person chose; the call's
                    // own ask stays on the manifest as `requestedModel`. A fork
                    // keeps its contract: it runs on the parent, unrouted.
                    if !forking {
                        if let Some(pin) = super::agent_tools::person_named_model(
                            ctx.turn_agent_policy().and_then(|policy| policy.user_named_model),
                            inp.model.as_deref(),
                        ) {
                            inp.route_model = Some(pin.model.to_string());
                            inp.route_source = Some(
                                super::smart_router::route_source_label(runtime::RouteDecisionSource::Pinned)
                                    .to_string(),
                            );
                            inp.route_reason = Some(match pin.corrected_from {
                                Some(asked) => {
                                    format!("named by the person in this turn (the call asked for {asked})")
                                }
                                None => "named by the person in this turn".to_string(),
                            });
                        }
                    }
                    // CC-style guard: fold a wasteful same-model, simple
                    // implementation spawn back to the main loop. The resolved
                    // model is what the spawn will ACTUALLY run — the global
                    // env override wins over everything (mirrors
                    // `try_resolve_agent_model_selection` /
                    // `smart_routed_model_selection`), then route → explicit →
                    // inherited parent. Custom-agent spawns are exempt inside
                    // the guard, so this non-custom precedence is exact.
                    let resolved_model: String =
                        std::env::var(super::agent_tools::AGENT_MODEL_ENV)
                            .ok()
                            .map(|value| value.trim().to_string())
                            .filter(|value| !value.is_empty())
                            .or_else(|| {
                                inp.route_model
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|m| !m.is_empty())
                                    .map(str::to_string)
                            })
                            .or_else(|| {
                                inp.model
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|m| !m.is_empty())
                                    .map(str::to_string)
                            })
                            .or_else(|| {
                                parent
                                    .as_deref()
                                    .map(str::trim)
                                    .filter(|m| !m.is_empty())
                                    .map(str::to_string)
                            })
                            .unwrap_or_default();
                    if ctx.person_model_pin().is_some_and(|pin|
                        super::canonicalize_route_model_id(&pin) == super::canonicalize_route_model_id(&resolved_model)) {
                        inp.route_source = Some(
                            super::smart_router::route_source_label(runtime::RouteDecisionSource::Pinned).to_string(),
                        );
                    }
                    let delegated = super::assess_agent_task(&inp.prompt);
                    let spawn_is_custom = spawn_uses_custom_agent(
                        explicit_subagent_type.as_deref(),
                        &inp.description,
                        &inp.prompt,
                    );
                    if !forking
                        && same_model_impl_spawn_is_wasteful(
                            &resolved_model,
                            parent.as_deref(),
                            delegated,
                            spawn_is_custom,
                            ctx.turn_agent_policy(),
                        )
                    {
                        return Ok(inline_fold_result(&resolved_model));
                    }
                    inp.route_reason = annotate_auto_type_reason(
                        inp.route_reason.take(),
                        auto_type.as_deref(),
                    );
                    inp.parent_session_id = ctx.session_id();
                    inp.registry = ctx.registry_handle();
                    inp.parent_permission_mode = active_parent_mode(ctx, enforcer);
                    inp.tool_call_id = smuggled_tool_call_id(input);
                    inp.mcp_passthrough = ctx.mcp_passthrough();
                    // The conversation a fork inherits — only a session host
                    // publishes one, so a fork from anywhere else is refused
                    // by the spawn with the reason.
                    inp.fork_source = forking.then(|| ctx.fork_source()).flatten();
                    // An omitted `background` defers to the host: detached in
                    // the interactive main session (its REPL re-injects the
                    // completion), blocking anywhere the result would be lost.
                    if inp.background.is_none() {
                        inp.background = Some(ctx.background_agent_default());
                    }
                    run_agent(
                        inp,
                        parent.as_deref(),
                        parent_lsp(ctx),
                        Some(ctx.hook_config()),
                    )
                })
            }),
        ),
        "ToolSearch" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<ToolSearchInput>(input).and_then(|inp| run_tool_search(&inp, ctx))
            }),
        ),
        // Read-only ledger rollup — takes no input, so it ignores `input`.
        "Audit" => Some(run_audit(ctx)),
        "session_recall" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<SessionRecallInput>(input).and_then(|inp| run_session_recall(inp, ctx))
            }),
        ),
        "retrieve_tool_output" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<RetrieveToolOutputInput>(input)
                    .and_then(|inp| run_retrieve_tool_output(&inp))
            }),
        ),
        "NotebookEdit" => Some(
            maybe_enforce_permission_check(enforcer, name, input)
                .and_then(|()| from_value::<NotebookEditInput>(input).and_then(run_notebook_edit)),
        ),
        "Sleep" => Some(from_value::<SleepInput>(input).and_then(|input| run_sleep(&input))),
        // `send_to_user` is the mid-run push tool; `SendUserMessage`/`Brief`
        // are legacy aliases kept for models trained on those names — all three
        // route through one runner so the behavior can never drift.
        "send_to_user" | "SendUserMessage" | "Brief" => Some(
            from_value::<SendToUserInput>(input).and_then(|inp| run_send_to_user(inp, ctx)),
        ),
        "PushNotification" => Some(
            from_value::<PushNotificationInput>(input)
                .and_then(|inp| run_push_notification(&inp, ctx)),
        ),
        "SyntheticOutput" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<SyntheticOutputInput>(input)
                    .and_then(|input| run_synthetic_output(&input))
            }),
        ),
        "StopAgent" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<StopAgentInput>(input)
                    .and_then(|inp| run_stop_agent(&ctx.registry(), &inp, ctx.session_id().as_deref()))
            }),
        ),
        "ListAgents" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<ListAgentsInput>(input).and_then(|inp| {
                    run_list_agents(&ctx.registry(), &inp, ctx.session_id().as_deref())
                })
            }),
        ),
        "SpawnMultiAgent" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<SpawnMultiAgentInput>(input).and_then(|mut inp| {
                    for member in &inp.agents {
                        refuse_small_spawn(member.get("prompt").and_then(Value::as_str).unwrap_or_default(), ctx.turn_agent_policy())?;
                    }
                    for member in &mut inp.agents {
                        if let Some(object) = member.as_object_mut() { object.retain(|key, _| !key.starts_with("__zo_route_")); }
                    }
                    let auto_types = stamp_inferred_spawn_types(&mut inp);
                    let parent = ctx.spawn_parent_model();
                    // Same explicit-model rule as the `Agent` arm above.
                    if !ctx.active_model_pinned() {
                        apply_smart_models_to_spawn_input_with_auto_types(
                            parent.as_deref(),
                            &mut inp,
                            &auto_types,
                        );
                    }
                    if let Some(pin) = ctx.person_model_pin() {
                        for member in &mut inp.agents {
                            let selected = member.get(ROUTE_MODEL_SMUGGLE_KEY).or_else(|| member.get("model")).and_then(Value::as_str).or(parent.as_deref());
                            if selected.is_some_and(|model| super::canonicalize_route_model_id(model) == super::canonicalize_route_model_id(&pin)) {
                                let meta = &mut member[super::smart_router::ROUTE_DECISION_META_SMUGGLE_KEY];
                                if !meta.is_object() { *meta = json!({}); }
                                meta["routeSource"] = json!("pin");
                            }
                        }
                    }
                    annotate_spawn_auto_type_reasons(&mut inp, &auto_types);
                    if let Some(resolved_model) = single_spawn_member_inline_fold_model(
                        &inp,
                        parent.as_deref(),
                        ctx.turn_agent_policy(),
                    ) {
                        return Ok(inline_fold_result(&resolved_model));
                    }
                    inp.parent_session_id = ctx.session_id();
                    inp.registry = ctx.registry_handle();
                    inp.parent_permission_mode = active_parent_mode(ctx, enforcer);
                    inp.tool_call_id = smuggled_tool_call_id(input);
                    inp.mcp_passthrough = ctx.mcp_passthrough();
                    run_spawn_multi_agent(
                        &inp,
                        parent.as_deref(),
                        parent_lsp(ctx),
                        Some(ctx.hook_config()),
                    )
                })
            }),
        ),
        "Council" => Some(from_value::<CouncilInput>(input).and_then(|inp| run_council(&inp))),
        "Config" => Some(
            maybe_enforce_permission_check(enforcer, name, input)
                .and_then(|()| from_value::<ConfigInput>(input).and_then(run_config)),
        ),
        "EnterPlanMode" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<EnterPlanModeInput>(input).and_then(run_enter_plan_mode)
            }),
        ),
        "ExitPlanMode" => Some(
            maybe_enforce_permission_check(enforcer, name, input)
                .map_err(|error| match error {
                    // Models trained on the CC-style tool of the same name call
                    // this to submit a plan; in read-only plan mode that denial
                    // is a dead end without a pointer to the real submission
                    // tool, costing a wasted round-trip per turn.
                    ToolError::PermissionDenied { tool, reason } => {
                        ToolError::PermissionDenied {
                            tool,
                            reason: format!(
                                "{reason} · To submit a plan for approval while in plan mode, call ExitPlanModeV2 (by name, or CapabilityInvoke({{name: \"ExitPlanModeV2\", input: {{plan}}}})) — plan mode is lifted only by the user."
                            ),
                        }
                    }
                    other => other,
                })
                .and_then(|()| from_value::<ExitPlanModeInput>(input).and_then(run_exit_plan_mode)),
        ),
        "StructuredOutput" => {
            Some(from_value::<StructuredOutputInput>(input).and_then(run_structured_output))
        }
        "AskUserQuestion" => Some(
            from_value::<AskUserQuestionInput>(input)
                .and_then(|inp| run_ask_user_question(inp, ctx.user_question_channel().as_deref())),
        ),
        "MemoryWrite" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<MemoryWriteInput>(input).and_then(|inp| run_memory_write(&inp, ctx))
            }),
        ),
        "RemoteTrigger" => Some(
            maybe_enforce_permission_check(enforcer, name, input).and_then(|()| {
                from_value::<RemoteTriggerInput>(input).and_then(run_remote_trigger)
            }),
        ),
        "Monitor" => Some(from_value::<MonitorInput>(input).and_then(run_monitor)),
        "SendMessage" => Some(from_value::<SendMessageInput>(input).and_then(|mut inp| {
            // A teammate NAME is session-scoped (CC semantics), so the lookup
            // needs the addressing session to prefer this session's agent over
            // a same-named worker left behind by another tab.
            inp.session_id = ctx.session_id();
            let caller = ctx.subagent_identity();
            // `SendMessage` is graded `DangerFullAccess` because its RESUME
            // path re-spawns a worker with the original permission envelope.
            // A sub-agent's restricted copy cannot reach that path at all
            // (`run_send_message` allows it exactly one target, `main`, which
            // only pushes text upstream), so gating the upstream edge on
            // full access would silently kill it for every read-only worker —
            // exactly the "feature dies quietly under a permission clamp"
            // failure. Every other caller/target still runs the check.
            let upstream_only =
                caller.is_some() && inp.to.trim().eq_ignore_ascii_case(MAIN_CONVERSATION_TARGET);
            if !upstream_only {
                maybe_enforce_permission_check(enforcer, name, input)?;
            }
            run_send_message(
                ctx.registry_handle(),
                &inp,
                caller,
                parent_lsp(ctx),
                Some(ctx.hook_config()),
                ctx.mcp_passthrough(),
                active_parent_mode(ctx, enforcer),
            )
        })),
        "ScheduleWakeup" => Some(
            from_value::<ScheduleWakeupInput>(input).and_then(|input| {
                let session_id = ctx.session_id();
                run_schedule_wakeup(&input, session_id.as_deref())
            }),
        ),
        _ => None,
    }
}

// ── Kernel bridge (`zo.*` inside the persistent REPL) ────────────────────────

/// Host servicing for kernel-side `zo.spawn`/`zo.result`, backed by THIS
/// dispatcher. Carrying the same `ctx`/`enforcer` the REPL call ran under is
/// the whole security story: a kernel spawn IS an `Agent` tool call with
/// `background: true` — permission-checked, routed, capped, and
/// manifest-tracked identically — never a private side door.
pub(crate) struct DispatchKernelBridge<'a> {
    pub(crate) ctx: &'a ToolContext,
    pub(crate) enforcer: Option<&'a PermissionEnforcer>,
}

impl crate::repl_kernel::KernelBridge for DispatchKernelBridge<'_> {
    fn call(&self, op: &str, args: &Value) -> Value {
        kernel_bridge_call(self.ctx, self.enforcer, op, args)
    }
}

fn kernel_bridge_call(
    ctx: &ToolContext,
    enforcer: Option<&PermissionEnforcer>,
    op: &str,
    args: &Value,
) -> Value {
    match op {
        "spawn" => kernel_bridge_spawn(ctx, enforcer, args),
        "result" => kernel_bridge_result(&ctx.registry(), args),
        "skill" => kernel_bridge_skill(args),
        "history" => kernel_bridge_history(ctx, args),
        other => serde_json::json!({
            "error": format!(
                "unknown zo bridge op `{other}` — this build supports spawn, result, skill, and history"
            )
        }),
    }
}

/// `zo.history(...)`: the session's own persisted transcript as data. Reads
/// the SAME autosave file `/resume` replays — no parallel history store —
/// so what kernel code sees is exactly what survives a restart. Main
/// transcript file only: rotated segments overlap the live file's compaction
/// summary, and including both would double-count the overlap.
fn kernel_bridge_history(ctx: &ToolContext, args: &Value) -> Value {
    let Some(session_id) = ctx.session_id() else {
        return serde_json::json!({
            "error": "zo.history: no session id in this REPL context (bare dispatch has no transcript)"
        });
    };
    let base = ctx
        .cwd
        .clone()
        .or_else(|| std::env::current_dir().ok())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let transcript = runtime::session_control::managed_session_search_dirs_for(&base)
        .into_iter()
        .map(|dir| dir.join(format!("{session_id}.jsonl")))
        .find(|path| path.is_file());
    let Some(path) = transcript else {
        return serde_json::json!({
            "error": format!("zo.history: no transcript found for session `{session_id}` yet")
        });
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return serde_json::json!({"error": "zo.history: the transcript is unreadable"});
    };
    let matches = history_matches(
        &text,
        args.get("contains").and_then(Value::as_str),
        args.get("role").and_then(Value::as_str),
        args.get("limit").and_then(Value::as_u64),
    );
    serde_json::json!({"result": matches})
}

/// Newest `limit` matching messages, returned in chronological order. Every
/// bound is enforced here so one call can never flood a cell: the limit is
/// clamped, each message's text is capped, and the assembled result is
/// trimmed oldest-first to the same byte budget every other bridge payload
/// honors.
fn history_matches(
    transcript: &str,
    contains: Option<&str>,
    role: Option<&str>,
    limit: Option<u64>,
) -> Vec<Value> {
    const TEXT_CAP_CHARS: usize = 4_000;
    const LIMIT_MAX: u64 = 100;
    let limit = usize::try_from(limit.unwrap_or(20).clamp(1, LIMIT_MAX)).unwrap_or(20);
    let needle = contains.map(str::to_lowercase);
    let role = role.map(str::to_lowercase);

    let mut matches: Vec<Value> = Vec::new();
    for line in transcript.lines() {
        let Ok(record) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if record.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(message) = record.get("message") else {
            continue;
        };
        let Some(message_role) = message.get("role").and_then(Value::as_str) else {
            continue;
        };
        if role.as_deref().is_some_and(|role| role != message_role) {
            continue;
        }
        let text = message_text(message);
        if text.is_empty() {
            continue;
        }
        if needle
            .as_deref()
            .is_some_and(|needle| !text.to_lowercase().contains(needle))
        {
            continue;
        }
        let mut text = text;
        if text.chars().count() > TEXT_CAP_CHARS {
            text = text.chars().take(TEXT_CAP_CHARS).collect::<String>() + "…(truncated)";
        }
        matches.push(serde_json::json!({"role": message_role, "text": text}));
    }
    // Newest `limit`, chronological order preserved by draining from the front.
    if matches.len() > limit {
        matches.drain(..matches.len() - limit);
    }
    // Total-budget trim, oldest first — same ceiling as every bridge payload.
    while matches.len() > 1
        && serde_json::to_string(&matches).map_or(0, |s| s.len())
            > crate::repl_kernel::BRIDGE_REPORT_MAX_BYTES
    {
        matches.remove(0);
    }
    matches
}

/// Concatenated text blocks of one transcript message (the same block shape
/// the session writer persists).
fn message_text(message: &Value) -> String {
    let blocks = message
        .get("blocks")
        .or_else(|| message.get("content"))
        .and_then(Value::as_array);
    let Some(blocks) = blocks else {
        return message
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
    };
    let mut out = String::new();
    for block in blocks {
        if let Some(text) = block.get("text").and_then(Value::as_str) {
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(text);
        }
    }
    out
}

/// `zo.skill(name)`: the resolved skill's instruction text plus its asset
/// directory. Goes through [`super::skill_tools::execute_skill`] — the same
/// loader the `Skill` tool uses — so the proposed-state review gate holds
/// identically for kernel code.
fn kernel_bridge_skill(args: &Value) -> Value {
    let Some(name) = args
        .get("name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
    else {
        return serde_json::json!({"error": "zo.skill requires a skill name"});
    };
    match super::skill_tools::execute_skill(super::skill_tools::SkillInput {
        skill: name.to_string(),
        args: None,
    }) {
        Ok(output) => {
            let dir = std::path::Path::new(&output.path)
                .parent()
                .map_or_else(String::new, |parent| parent.display().to_string());
            serde_json::json!({"result": {
                "name": output.skill,
                "origin": output.origin,
                "dir": dir,
                "prompt": output.prompt,
            }})
        }
        Err(error) => serde_json::json!({"error": format!("zo.skill failed: {error}")}),
    }
}

fn kernel_bridge_spawn(
    ctx: &ToolContext,
    enforcer: Option<&PermissionEnforcer>,
    args: &Value,
) -> Value {
    let Some(prompt) = args
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|prompt| !prompt.trim().is_empty())
    else {
        return serde_json::json!({"error": "zo.spawn requires a non-empty prompt"});
    };
    let mut agent_input = serde_json::Map::new();
    agent_input.insert("prompt".to_string(), Value::String(prompt.to_string()));
    // The Python side always derives a description; falling back to the
    // prompt keeps a hand-built frame valid without inventing another label.
    let description = args
        .get("description")
        .and_then(Value::as_str)
        .filter(|description| !description.trim().is_empty())
        .unwrap_or(prompt);
    agent_input.insert("description".to_string(), Value::String(description.to_string()));
    for key in ["subagent_type", "model"] {
        if let Some(value) = args.get(key).and_then(Value::as_str) {
            if !value.trim().is_empty() {
                agent_input.insert(key.to_string(), Value::String(value.to_string()));
            }
        }
    }
    // Detach at spawn: the kernel gets a handle back immediately and collects
    // the report in a LATER cell via `zo.result`, so a cell never blocks for
    // agent-length time inside its own exec deadline.
    agent_input.insert("background".to_string(), Value::Bool(true));
    match dispatch(ctx, enforcer, "Agent", &Value::Object(agent_input)) {
        Some(Ok(text)) => {
            let parsed = serde_json::from_str::<Value>(&text).unwrap_or(Value::String(text));
            serde_json::json!({"result": parsed})
        }
        Some(Err(error)) => serde_json::json!({"error": format!("zo.spawn failed: {error}")}),
        None => serde_json::json!({"error": "the Agent tool is not available in this dispatcher"}),
    }
}

fn kernel_bridge_result(registry: &super::agent_tools::AgentRegistry, args: &Value) -> Value {
    let Some(agent_id) = args
        .get("agent_id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
    else {
        return serde_json::json!({"error": "zo.result requires an agent_id"});
    };
    // The id becomes a store file name; restricting it to the id alphabet the
    // spawn paths produce keeps kernel input from ever walking the path.
    if !agent_id
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
    {
        return serde_json::json!({
            "error": "zo.result: agent_id must be a plain agent identifier"
        });
    }
    let Some(manifest) = super::agent_tools::agent_manifest_by_id(registry, agent_id) else {
        return serde_json::json!({
            "error": format!("no agent manifest found for `{agent_id}`")
        });
    };
    let (report, report_truncated) = kernel_bridge_report(&manifest.output_file);
    serde_json::json!({"result": {
        "agentId": manifest.agent_id,
        "name": manifest.name,
        "status": manifest.status,
        "resolvedModel": manifest.resolved_model,
        "report": report,
        "reportTruncated": report_truncated,
    }})
}

#[cfg(test)]
mod kernel_bridge_tests {
    use super::*;

    #[test]
    fn bridge_rejects_unknown_ops_and_path_shaped_agent_ids() {
        let registry = crate::misc_tools::AgentRegistry::unowned_from_cwd();
        let unknown = kernel_bridge_result(&registry, &serde_json::json!({}));
        assert!(
            unknown
                .get("error")
                .and_then(Value::as_str)
                .is_some_and(|error| error.contains("requires an agent_id")),
            "{unknown}"
        );
        for hostile in ["../secrets", "a/b", "a\\b", "agent id", "x.json"] {
            let denied = kernel_bridge_result(&registry, &serde_json::json!({"agent_id": hostile}));
            assert!(
                denied
                    .get("error")
                    .and_then(Value::as_str)
                    .is_some_and(|error| error.contains("plain agent identifier")),
                "{hostile} must be rejected: {denied}"
            );
        }
    }

    #[test]
    fn history_matches_filters_clamps_and_orders_chronologically() {
        let line = |role: &str, text: &str| {
            serde_json::json!({
                "type": "message",
                "message": {"role": role, "blocks": [{"type": "text", "text": text}]},
            })
            .to_string()
        };
        let transcript = [
            line("user", "fix the login bug"),
            line("assistant", "found the login handler"),
            line("tool", "grep output about sessions"),
            line("user", "now fix the logout path"),
            serde_json::json!({"type": "session_meta"}).to_string(),
            "not json at all".to_string(),
        ]
        .join("\n");

        let all = history_matches(&transcript, None, None, None);
        assert_eq!(all.len(), 4, "meta and garbage lines are skipped");
        assert_eq!(all[0]["text"], "fix the login bug", "chronological order");

        let user_only = history_matches(&transcript, None, Some("user"), None);
        assert_eq!(user_only.len(), 2);

        let login = history_matches(&transcript, Some("LOGIN"), None, None);
        assert_eq!(login.len(), 2, "contains is case-insensitive");

        let last_one = history_matches(&transcript, None, None, Some(1));
        assert_eq!(last_one.len(), 1);
        assert_eq!(last_one[0]["text"], "now fix the logout path", "newest wins");

        let huge = line("assistant", &"x".repeat(10_000));
        let capped = history_matches(&huge, None, None, None);
        assert!(
            capped[0]["text"]
                .as_str()
                .expect("text")
                .ends_with("…(truncated)"),
            "per-message text cap applies"
        );
    }

    #[test]
    fn bridge_skill_op_validates_and_reports_loader_errors() {
        let missing_name = kernel_bridge_skill(&serde_json::json!({}));
        assert!(
            missing_name
                .get("error")
                .and_then(Value::as_str)
                .is_some_and(|error| error.contains("requires a skill name")),
            "{missing_name}"
        );
        // A name no catalog can resolve surfaces the loader's own error —
        // the bridge adds transport, never a second resolution path.
        let unknown = kernel_bridge_skill(&serde_json::json!({
            "name": format!("zo-test-nonexistent-skill-{}", std::process::id()),
        }));
        assert!(
            unknown
                .get("error")
                .and_then(Value::as_str)
                .is_some_and(|error| error.contains("unknown skill")),
            "{unknown}"
        );
    }

    #[test]
    fn bridge_report_caps_and_signals_truncation() {
        let dir = std::env::temp_dir().join(format!("zo-bridge-report-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("report.md");
        std::fs::write(&path, "b".repeat(crate::repl_kernel::BRIDGE_REPORT_MAX_BYTES + 100))
            .expect("write report");
        let (report, truncated) = kernel_bridge_report(&path.display().to_string());
        assert!(truncated);
        assert_eq!(
            report.as_str().map(str::len),
            Some(crate::repl_kernel::BRIDGE_REPORT_MAX_BYTES)
        );
        let (missing, missing_truncated) = kernel_bridge_report("");
        assert_eq!(missing, Value::Null);
        assert!(!missing_truncated);
        let _ = std::fs::remove_file(&path);
    }
}

/// The agent's report file as a JSON value, `Null` while absent or empty (an
/// agent that has not written yet), capped at the same byte budget the REPL
/// applies to its own captured streams so one report cannot flood a cell.
fn kernel_bridge_report(path: &str) -> (Value, bool) {
    if path.trim().is_empty() {
        return (Value::Null, false);
    }
    match std::fs::read_to_string(path) {
        Ok(text) if text.trim().is_empty() => (Value::Null, false),
        Ok(mut text) => {
            let cap = crate::repl_kernel::BRIDGE_REPORT_MAX_BYTES;
            if text.len() > cap {
                let mut end = cap;
                while !text.is_char_boundary(end) {
                    end -= 1;
                }
                text.truncate(end);
                (Value::String(text), true)
            } else {
                (Value::String(text), false)
            }
        }
        Err(_) => (Value::Null, false),
    }
}

#[cfg(test)]
mod tests {
    use std::time::{SystemTime, UNIX_EPOCH};

    use serde_json::json;

    use super::*;

    struct EnvGuard {
        key: &'static str,
        prior: Option<std::ffi::OsString>,
    }

    impl EnvGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let prior = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, prior }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.prior.take() {
                Some(value) => std::env::set_var(self.key, value),
                None => std::env::remove_var(self.key),
            }
        }
    }

    #[test]
    fn execution_quality_small_read_only_delegation_requires_the_persons_request() {
        let prompt = "Check the git remote.";
        assert_eq!(super::super::assess_agent_task(prompt).complexity, runtime::RouteTaskComplexity::Small);
        assert!(small_spawn_is_wasteful(prompt, Some(solo_simple_policy())));
        assert!(!small_spawn_is_wasteful(prompt, Some(crate::TurnAgentPolicy { user_requested_delegation: true, ..solo_simple_policy() })));
        assert!(!small_spawn_is_wasteful(prompt, None));
    }

    fn solo_simple_policy() -> crate::TurnAgentPolicy {
        crate::TurnAgentPolicy {
            user_complexity: runtime::RouteTaskComplexity::Small,
            user_shape: runtime::RouteShapeKind::Solo,
            user_need_count: 0,
            user_requested_delegation: false,
            user_named_model: None,
        }
    }

    fn write_simple_task() -> crate::AgentTaskAssessment {
        crate::AgentTaskAssessment {
            complexity: runtime::RouteTaskComplexity::Small,
            has_write_intent: true,
        }
    }

    /// The guard folds ONLY a same-model, simple, generic, non-delegation-worthy
    /// implementation spawn on a simple Solo turn; every genuine reason to spawn
    /// keeps it, and unknown per-turn policy fails open.
    #[test]
    fn same_model_impl_spawn_guard_folds_only_the_wasteful_case() {
        let sol = Some("gpt-5.6-sol");
        let policy = Some(solo_simple_policy());
        let task = write_simple_task();

        // Baseline: same model, simple write slice, no custom, simple Solo turn.
        assert!(same_model_impl_spawn_is_wasteful(
            "gpt-5.6-sol",
            sol,
            task,
            false,
            policy,
        ));
        // Fail open — unknown per-turn policy (background / non-turn dispatch).
        assert!(!same_model_impl_spawn_is_wasteful(
            "gpt-5.6-sol",
            sol,
            task,
            false,
            None,
        ));
        // Escape — the user turn itself warrants delegation (non-Solo shape).
        assert!(!same_model_impl_spawn_is_wasteful(
            "gpt-5.6-sol",
            sol,
            task,
            false,
            Some(crate::TurnAgentPolicy {
                user_shape: runtime::RouteShapeKind::ParallelLanes,
                ..solo_simple_policy()
            }),
        ));
        // Escape — the user turn is hard (complex whole-turn).
        assert!(!same_model_impl_spawn_is_wasteful(
            "gpt-5.6-sol",
            sol,
            task,
            false,
            Some(crate::TurnAgentPolicy {
                user_complexity: runtime::RouteTaskComplexity::Large,
                ..solo_simple_policy()
            }),
        ));
        // Escape — the turn has planned agent needs.
        assert!(!same_model_impl_spawn_is_wasteful(
            "gpt-5.6-sol",
            sol,
            task,
            false,
            Some(crate::TurnAgentPolicy {
                user_need_count: 2,
                ..solo_simple_policy()
            }),
        ));
        // Escape — the user EXPLICITLY requested delegation (non-Solo requested
        // shape), even though the natural shape is Solo with no needs.
        assert!(!same_model_impl_spawn_is_wasteful(
            "gpt-5.6-sol",
            sol,
            task,
            false,
            Some(crate::TurnAgentPolicy {
                user_requested_delegation: true,
                ..solo_simple_policy()
            }),
        ));
        // Escape — a user-defined custom agent.
        assert!(!same_model_impl_spawn_is_wasteful(
            "gpt-5.6-sol", sol, task, true, policy,
        ));
        // Escape — a genuinely different effective model.
        assert!(!same_model_impl_spawn_is_wasteful(
            "claude-opus-4-8",
            sol,
            task,
            false,
            policy,
        ));
        // Escape — read-only / research delegated work.
        assert!(!same_model_impl_spawn_is_wasteful(
            "gpt-5.6-sol",
            sol,
            crate::AgentTaskAssessment {
                complexity: runtime::RouteTaskComplexity::Small,
                has_write_intent: false,
            },
            false,
            policy,
        ));
        // Escape — a genuinely hard delegated slice.
        assert!(!same_model_impl_spawn_is_wasteful(
            "gpt-5.6-sol",
            sol,
            crate::AgentTaskAssessment {
                complexity: runtime::RouteTaskComplexity::Large,
                has_write_intent: true,
            },
            false,
            policy,
        ));
        // Escape — no parent model (non-live/test harness).
        assert!(!same_model_impl_spawn_is_wasteful(
            "gpt-5.6-sol", None, task, false, policy,
        ));
    }

    #[test]
    fn single_spawn_member_guard_folds_only_a_wasteful_solo_member() {
        let _lock = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let _model = EnvGuard::set(super::super::agent_tools::AGENT_MODEL_ENV, "");
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "zo-dispatch-single-spawn-custom-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("custom agent dir");
        std::fs::write(
            dir.join("myrefactorer.md"),
            "---\nname: myrefactorer\ndescription: Custom\ntools: edit_file\n---\nRefactor locally.",
        )
        .expect("custom agent definition");
        let _defs = EnvGuard::set("ZO_AGENT_DEFS_DIR", &dir);
        let spawn = |agents: Vec<Value>| -> SpawnMultiAgentInput {
            serde_json::from_value(json!({ "agents": agents })).expect("spawn input")
        };
        let write_prompt = "Write a Rust function for the endpoint";
        assert_eq!(
            super::super::assess_agent_task(write_prompt),
            write_simple_task()
        );

        let same_model = json!({
            "prompt": write_prompt,
            "subagent_type": "general-purpose",
            "model": "claude-opus-4-8",
            "__zo_route_model": "gpt-5.6-sol"
        });
        assert_eq!(
            single_spawn_member_inline_fold_model(
                &spawn(vec![same_model.clone()]),
                Some("gpt-5.6-sol"),
                Some(solo_simple_policy()),
            )
            .as_deref(),
            Some("gpt-5.6-sol"),
            "the routed model wins over the member model and folds"
        );
        assert_eq!(
            single_spawn_member_inline_fold_model(
                &spawn(vec![same_model.clone(), same_model]),
                Some("gpt-5.6-sol"),
                Some(solo_simple_policy()),
            ),
            None,
            "two members are a real swarm"
        );
        assert_eq!(
            single_spawn_member_inline_fold_model(
                &spawn(vec![json!({
                    "prompt": write_prompt,
                    "subagent_type": "general-purpose",
                    "__zo_route_model": "claude-opus-4-8"
                })]),
                Some("gpt-5.6-sol"),
                Some(solo_simple_policy()),
            ),
            None,
            "a different routed model is genuine diversity"
        );
        assert_eq!(
            single_spawn_member_inline_fold_model(
                &spawn(vec![json!({
                    "prompt": write_prompt,
                    "subagent_type": "myrefactorer",
                    "__zo_route_model": "gpt-5.6-sol"
                })]),
                Some("gpt-5.6-sol"),
                Some(solo_simple_policy()),
            ),
            None,
            "a custom agent remains isolated"
        );
        assert_eq!(
            single_spawn_member_inline_fold_model(
                &spawn(vec![json!({
                    "prompt": "Inspect dispatch.rs and report the relevant code",
                    "subagent_type": "Explore",
                    "__zo_route_model": "gpt-5.6-sol"
                })]),
                Some("gpt-5.6-sol"),
                Some(solo_simple_policy()),
            ),
            None,
            "read-only work remains delegated"
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    /// Only a user-defined CUSTOM agent (.md) counts as a specialization. No
    /// explicit type, generic aliases, built-in labels, and invented names all
    /// lack a custom definition, so the guard cannot be dodged with a made-up
    /// `subagent_type`.
    #[test]
    fn spawn_uses_custom_agent_only_for_real_custom_definitions() {
        let _lock = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "zo-dispatch-custom-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("custom agent dir");
        std::fs::write(
            dir.join("myrefactorer.md"),
            "---\nname: myrefactorer\ndescription: Custom\ntools: edit_file\n---\nRefactor locally.",
        )
        .expect("custom agent definition");
        let _defs = EnvGuard::set("ZO_AGENT_DEFS_DIR", &dir);

        assert!(!spawn_uses_custom_agent(None, "", ""));
        assert!(!spawn_uses_custom_agent(Some("general-purpose"), "", ""));
        assert!(!spawn_uses_custom_agent(Some("general"), "", ""));
        assert!(!spawn_uses_custom_agent(Some("implementer"), "", ""));
        assert!(!spawn_uses_custom_agent(Some("code-reviewer"), "", ""));
        assert!(spawn_uses_custom_agent(Some("myrefactorer"), "", ""));

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn untyped_agent_is_stamped_before_routing() {
        let mut input: AgentInput = serde_json::from_value(json!({
            "description": "Debug the parser crash",
            "prompt": "reproduce the panic, find the root cause, and fix the bug"
        }))
        .expect("agent input");

        let auto_type = stamp_inferred_agent_type(&mut input);

        assert_eq!(auto_type.as_deref(), Some("debugger"));
        assert_eq!(input.subagent_type.as_deref(), Some("debugger"));
        let role = runtime::SubagentProfileId::parse("debugger")
            .and_then(|profile| profile.route_role_hint());
        assert_eq!(role, Some(runtime::RouteRole::Debugging));
        assert_eq!(
            super::super::agent_tools::display_agent_label(
                Some("Fix parser"),
                &input.description,
                "fix-parser",
                input.subagent_type.as_deref().expect("stamped type"),
            ),
            Some("debugger·Fix parser".to_string())
        );
        assert_eq!(
            annotate_auto_type_reason(None, auto_type.as_deref()).as_deref(),
            Some("type=debugger (auto)")
        );
    }

    #[test]
    fn spawn_members_stamp_only_missing_types() {
        let mut input: SpawnMultiAgentInput = serde_json::from_value(json!({
            "agents": [
                {
                    "description": "Find where sessions are loaded",
                    "prompt": "explore the codebase and return file references"
                },
                {
                    "description": "Classify task",
                    "prompt": "return one label",
                    "subagent_type": "classifier"
                }
            ]
        }))
        .expect("spawn input");

        let auto_types = stamp_inferred_spawn_types(&mut input);
        annotate_spawn_auto_type_reasons(&mut input, &auto_types);

        assert_eq!(input.agents[0]["subagent_type"], "Explore");
        assert_eq!(input.agents[1]["subagent_type"], "classifier");
        assert_eq!(auto_types[0].as_deref(), Some("Explore"));
        assert_eq!(auto_types[1], None);
        assert_eq!(
            input.agents[0][ROUTE_REASON_SMUGGLE_KEY],
            "type=Explore (auto)"
        );
        assert!(input.agents[1].get(ROUTE_REASON_SMUGGLE_KEY).is_none());
    }

    #[test]
    fn explicit_custom_type_survives_dispatch_and_job_resolution() {
        let _lock = crate::tests::env_lock()
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "zo-dispatch-custom-agent-{}-{unique}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("custom agent dir");
        std::fs::write(
            dir.join("reviewer.md"),
            "---\nname: reviewer\ndescription: Custom reviewer\ntools: read_file\n---\nReview locally.",
        )
        .expect("custom agent definition");
        let _defs = EnvGuard::set("ZO_AGENT_DEFS_DIR", &dir);
        let mut input: AgentInput = serde_json::from_value(json!({
            "description": "Review the patch",
            "prompt": "review this code",
            "subagent_type": "reviewer"
        }))
        .expect("agent input");

        assert_eq!(stamp_inferred_agent_type(&mut input), None);
        let (resolved, custom) = super::super::agent_tools::resolve_subagent_type_and_custom_agent(
            input.subagent_type.as_deref(),
            &input.description,
            &input.prompt,
        );

        assert_eq!(resolved, "reviewer");
        assert_eq!(custom.expect("custom definition wins").name, "reviewer");
        let _ = std::fs::remove_dir_all(dir);
    }
}
