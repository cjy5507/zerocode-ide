use core_types::helper_run::HelperRun;

/// 「fable 검토 받아」 with the call asking for Opus: the person's name wins
/// and the door knows what it overrode. No name in the turn → no pin, the
/// ordinary precedence runs. The same model asked two ways is no correction.
#[test]
fn a_model_the_person_named_outranks_the_call_and_names_what_it_overrode() {
    let fable = api::resolve_model_alias(api::ANTHROPIC_FABLE_MODEL_ALIAS);
    let fable: &'static str = Box::leak(fable.into_boxed_str());
    let opus = api::resolve_model_alias("opus");

    let pinned = person_named_model(Some(fable), Some(&opus)).expect("the turn named a model");
    assert_eq!(pinned.model, fable);
    assert_eq!(pinned.corrected_from.as_deref(), Some(opus.as_str()));

    assert_eq!(person_named_model(None, Some(&opus)), None);

    let agreed = person_named_model(Some(fable), Some(api::ANTHROPIC_FABLE_MODEL_ALIAS))
        .expect("the turn named a model");
    assert_eq!(agreed.corrected_from, None, "the same model asked by nickname is not a correction");
    assert_eq!(person_named_model(Some(fable), None).expect("pin").corrected_from, None);
}
use super::subagent_profile::{
    builtin_harness_instruction, resolve_agent_model, resolve_agent_model_selection,
    try_resolve_agent_model_selection,
};
use super::*;

use std::path::PathBuf;
use crate::tests::EnvGuard;

fn latest_google_model() -> &'static str {
    api::latest_model_for_provider(api::ProviderKind::Google)
        .expect("provider catalog must define the current Google head")
}

struct TempAgentStore {
    path: PathBuf,
}

impl TempAgentStore {
    fn new(test_name: &str) -> Self {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "zo-agent-tools-{test_name}-{nanos}-{}",
            std::process::id()
        ));
        Self { path }
    }
}

impl Drop for TempAgentStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn sample_agent_input() -> AgentInput {
    AgentInput {
        route_probe_confidence: None,
        fork_source: None,
        allow_cross_provider: false,
        description: "Inspect the UI".to_string(),
        prompt: "Review the startup screen.".to_string(),
        subagent_type: Some("general-purpose".to_string()),
        name: Some("ui-review".to_string()),
        addressable_name: None,
        model: None,
        cwd: None,
        schema: None,
        background: Some(false),
        workflow_member: false,
        one_shot: false,
        plan_shape: None,
        route_tax: None,
        api_concurrency: None,
        parent_permission_mode: None,
        parent_session_id: None,
        registry: None,
        tool_call_id: None,
        mcp_passthrough: None,
        time_budget: None,
        prior_failures: 0,
        route_reason: None,
        route_role: None,
        route_complexity: None,
        route_risk: None,
        route_source: None,
        route_model: None,
        route_fallback_models: Vec::new(),
        route_effort: None,
        judged_agent: None,
        verify_loop: None,
        sees: Vec::new(),
    }
}

#[test]
fn resolve_agent_model_ignores_explicit_value_without_parent() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);

    assert_eq!(
        resolve_agent_model(Some(" explicit-model "), None, "my-custom-agent", None),
        DEFAULT_AGENT_MODEL
    );
}

#[test]
fn resolve_agent_model_uses_env_before_default() {
    let _guard = EnvGuard::set(AGENT_MODEL_ENV, " env-model ");

    assert_eq!(
        resolve_agent_model(None, None, "my-custom-agent", Some("parent-model")),
        "env-model"
    );
}

#[test]
fn resolve_agent_model_inherits_parent_before_default() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);

    assert_eq!(
        resolve_agent_model(None, None, "my-custom-agent", Some(" parent-model ")),
        "parent-model"
    );
    assert_eq!(
        resolve_agent_model(None, None, "Explore", Some("gpt-5.5-2026-04-23")),
        "gpt-5.5-2026-04-23"
    );
}

#[test]
fn active_model_runtime_switch_propagates_to_spawn_parent() {
    // Proves the bug-2 fix: a `/model` switch updates `ToolContext::active_model`
    // (a shared `Arc<Mutex>` cell) and the spawn dispatch reads the NEW model
    // through `spawn_parent_model()` → `resolve_agent_model`. Before the fix the
    // executor's registry clone was frozen at the build-time (startup) model, so
    // sub-agents kept spawning on the old model after a switch.
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV); // an env override would mask the parent

    let ctx = crate::context::ToolContext::new();
    ctx.set_active_model("claude-opus-4-8");
    assert_eq!(
        resolve_agent_model(None, None, "Explore", ctx.spawn_parent_model().as_deref()),
        "claude-opus-4-8"
    );

    // Simulate `/model gpt-5.5` at runtime — the same cell, re-set.
    ctx.set_active_model("gpt-5.5-2026-04-23");
    assert_eq!(
        resolve_agent_model(None, None, "Explore", ctx.spawn_parent_model().as_deref()),
        "gpt-5.5-2026-04-23"
    );

    // Blank input clears the cell → spawn falls back to the process default.
    ctx.set_active_model("   ");
    assert_eq!(
        resolve_agent_model(None, None, "Explore", ctx.spawn_parent_model().as_deref()),
        DEFAULT_AGENT_MODEL
    );
}

#[test]
fn spawn_parent_model_is_active_model_only() {
    // Sub-agent routing inherits the live foreground model. Role-specific or
    // cross-provider selection belongs to `/smart`, not to a hidden session-wide override.
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);

    let ctx = crate::context::ToolContext::new();
    ctx.set_active_model("claude-opus-4-8");
    assert_eq!(
        resolve_agent_model(None, None, "Explore", ctx.spawn_parent_model().as_deref()),
        "claude-opus-4-8"
    );

    ctx.set_active_model("gpt-5.5");
    assert_eq!(
        resolve_agent_model(None, None, "Explore", ctx.spawn_parent_model().as_deref()),
        "gpt-5.5"
    );

    ctx.set_active_model("   ");
    assert_eq!(
        resolve_agent_model(None, None, "Explore", ctx.spawn_parent_model().as_deref()),
        DEFAULT_AGENT_MODEL
    );
}

#[test]
fn resolve_agent_model_ignores_per_agent_model_metadata() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);

    // A file-based custom agent's `model` is legacy metadata. Live agents keep
    // the active parent/session model instead of crossing model families.
    assert_eq!(
        resolve_agent_model(
            None,
            Some(" custom-model "),
            "my-custom-agent",
            Some("parent-model")
        ),
        "parent-model"
    );
    assert_eq!(
        resolve_agent_model(
            Some("explicit"),
            Some("custom-model"),
            "my-custom-agent",
            Some("parent")
        ),
        "parent"
    );
    assert_eq!(
        resolve_agent_model(None, Some("custom-model"), "my-custom-agent", None),
        DEFAULT_AGENT_MODEL
    );
}

#[test]
fn resolve_agent_model_falls_back_to_default() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);

    assert_eq!(
        resolve_agent_model(Some("  "), None, "my-custom-agent", Some("  ")),
        DEFAULT_AGENT_MODEL
    );
}

#[test]
fn resolve_agent_model_inherits_parent_for_every_harness_type() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);

    for role in [
        "Plan",
        "Verification",
        "Explore",
        "general-purpose",
        "zo-guide",
        "deep-research",
        "code-reviewer",
        "debugger",
        "data-analyst",
        "refactor",
        "my-custom-agent",
    ] {
        assert_eq!(
            resolve_agent_model(Some("sonnet"), Some("opus"), role, Some("gpt-5.5")),
            "gpt-5.5",
            "{role} should inherit the parent model"
        );
    }
}

#[test]
fn fast_openai_work_inherits_parent_without_thinking_budget() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);

    let selection = resolve_agent_model_selection(
        None,
        None,
        "Explore",
        Some("gpt-5.5-2026-04-23"),
        "Find auth code",
        "Locate the files that define auth routing.",
    );

    // CC parity: the sub-agent runs on the parent's model verbatim; quick work
    // just skips the extended reasoning budget.
    assert_eq!(selection.model, "gpt-5.5-2026-04-23");
    assert_eq!(selection.thinking_budget_tokens, None);
}

#[test]
fn generic_gpt_parent_alias_is_inherited_verbatim() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);

    let selection = resolve_agent_model_selection(
        None,
        None,
        "debugger",
        Some("gpt"),
        "Debug runtime streaming",
        "Find the root cause, fix the parser, and verify the stream contract.",
    );

    // Alias resolution happens downstream (provider client); the routing layer
    // inherits the parent token as-is and only assigns the reasoning budget.
    assert_eq!(selection.model, "gpt");
    assert_eq!(selection.thinking_budget_tokens, Some(10_000));
}

#[test]
fn openai_parents_inherit_with_difficulty_scaled_budget() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);

    let standard = resolve_agent_model_selection(
        None,
        None,
        "data-analyst",
        Some("gpt-5.5"),
        "Analyze metric logs",
        "Compute the aggregate statistics from the logs.",
    );
    assert_eq!(standard.model, "gpt-5.5");
    assert_eq!(standard.thinking_budget_tokens, Some(4_096));

    let complex = resolve_agent_model_selection(
        None,
        None,
        "Plan",
        Some("gpt-5.6-luna"),
        "Design provider adapter structure",
        "Propose the architecture and tradeoffs.",
    );
    assert_eq!(complex.model, "gpt-5.6-luna");
    assert_eq!(complex.thinking_budget_tokens, Some(10_000));

    let hard = resolve_agent_model_selection(
        None,
        None,
        "debugger",
        Some("gpt-5.6-luna"),
        "Debug failing cache policy",
        "Find the root cause, fix the bug, and verify the request shape.",
    );
    assert_eq!(hard.model, "gpt-5.6-luna");
    assert_eq!(hard.thinking_budget_tokens, Some(10_000));

    let deep = resolve_agent_model_selection(
        None,
        None,
        "deep-research",
        Some("gpt-5.3-codex-spark"),
        "Deep research provider architecture",
        "Do a comprehensive analysis and compare the tradeoffs.",
    );
    assert_eq!(deep.model, "gpt-5.3-codex-spark");
    assert_eq!(deep.thinking_budget_tokens, Some(16_000));
}

#[test]
fn anthropic_parent_is_inherited_and_env_override_wins() {
    let guard = EnvGuard::clear(AGENT_MODEL_ENV);
    // CC parity (2026-06-11): a hard debugger task under a Claude parent stays
    // on the parent model — only the extended-thinking budget escalates.
    let hard = resolve_agent_model_selection(
        None,
        None,
        "debugger",
        Some("claude-opus-4-8"),
        "Debug hard bug",
        "Find the root cause.",
    );
    assert_eq!(hard.model, "claude-opus-4-8");
    assert_eq!(hard.thinking_budget_tokens, Some(10_000));

    drop(guard);
    let _guard = EnvGuard::set(AGENT_MODEL_ENV, " custom-agent-model ");
    let forced = resolve_agent_model_selection(
        None,
        None,
        "Explore",
        Some("gpt-5.5"),
        "Find files",
        "Locate the implementation.",
    );
    assert_eq!(forced.model, "custom-agent-model");
    assert_eq!(forced.thinking_budget_tokens, None);
}

#[test]
fn anthropic_small_task_inherits_parent_without_budget() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);
    let selection = resolve_agent_model_selection(
        None,
        None,
        "general-purpose",
        Some("claude-opus-4-8"),
        "Summarize a file",
        "Give a one-paragraph summary.",
    );
    assert_eq!(
        selection.model, "claude-opus-4-8",
        "sub-agents run on the parent model (CC parity), even for small work"
    );
    assert_eq!(selection.thinking_budget_tokens, None);
}

#[test]
fn anthropic_complex_task_inherits_parent_with_high_budget() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);
    let selection = resolve_agent_model_selection(
        None,
        None,
        "code-reviewer",
        Some("claude-sonnet-4-6"),
        "Review a security-critical change",
        "Find correctness and security bugs.",
    );
    assert_eq!(
        selection.model, "claude-sonnet-4-6",
        "hard Anthropic work keeps the parent model and earns a thinking budget"
    );
    assert_eq!(selection.thinking_budget_tokens, Some(10_000));
}

#[test]
fn live_spawn_rejects_explicit_cross_provider_model_instead_of_falling_back() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);

    let error = try_resolve_agent_model_selection(
        Some("gemini-3.5-flash"),
        false,
        None,
        "general-purpose",
        Some("gpt-5.5-fast"),
        "Implement a UI change",
        "Do the coding task.",
    )
    .expect_err("explicit cross-provider model must fail fast");
    let message = error.to_string();
    assert!(message.contains("gemini-3.5-flash"), "{message}");
    assert!(message.contains("gpt-5.5-fast"), "{message}");
    assert!(
        message.contains("do this work inline"),
        "{message}"
    );
    assert!(message.contains("Do not retry this spawn"), "{message}");
    assert!(!message.contains("\"allow_cross_provider\": true"), "{message}");
}

/// `allow_cross_provider: true` — the explicit, transcript-visible escape
/// hatch — honors the requested family through semantic catalog resolution.
#[test]
fn person_custom_model_pin_promotes_unqualified_cross_provider_model() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);

    let selection = try_resolve_agent_model_selection(
        None,
        false,
        Some("claude-opus-4-8"),
        "general-purpose",
        Some("gpt-5.6-sol"),
        "Implement the manifest runner",
        "Do the coding task on the model the user asked for.",
    )
    .expect("user-approved cross-provider model must be honored verbatim");
    assert_eq!(selection.model, api::latest_anthropic_model());
}

#[test]
fn live_spawn_snaps_unregistered_explicit_claude_version() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);

    let selection = try_resolve_agent_model_selection(
        Some("claude-opus-4.6"),
        true,
        None,
        "general-purpose",
        Some("claude-sonnet-4-6"),
        "Implement the manifest runner",
        "Do the coding task on Opus.",
    )
    .expect("user-approved Claude model must resolve to a registered spawn target");
    assert_eq!(selection.model, "claude-opus-5");
}

#[test]
fn live_spawn_still_honors_same_family_model_and_env_override() {
    let guard = EnvGuard::clear(AGENT_MODEL_ENV);

    let same = try_resolve_agent_model_selection(
        Some("gemini-3.5-flash"),
        false,
        None,
        "general-purpose",
        Some("gemini-3.5-pro"),
        "do a task",
        "do a task",
    )
    .expect("same provider family model should be honored");
    assert_eq!(same.model, latest_google_model());

    drop(guard);
    let _guard = EnvGuard::set(AGENT_MODEL_ENV, "forced-agent-model");
    let forced = try_resolve_agent_model_selection(
        Some("gemini-3.5-flash"),
        false,
        None,
        "general-purpose",
        Some("gpt-5.5-fast"),
        "do a task",
        "do a task",
    )
    .expect("ZO_AGENT_MODEL intentionally overrides all agents");
    assert_eq!(forced.model, "forced-agent-model");
}

#[test]
fn live_spawn_path_rejects_explicit_cross_provider_before_spawn() {
    let store = TempAgentStore::new("reject-cross-provider");
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV)
        .set_also(super::labels::AGENT_STORE_ENV, store.path.as_os_str());
    let mut input = sample_agent_input();
    input.model = Some("gemini-3.5-flash".to_string());

    let error = execute_agent_with_spawn_and_parent_model(
        input,
        |_| panic!("spawn_fn must not be called after model selection fails"),
        Some("gpt-5.5-fast"),
        None,
    )
    .expect_err("cross-provider explicit model should fail before spawn");

    match error {
        ToolError::InvalidInput(message) => {
            assert!(message.contains("gemini-3.5-flash"), "{message}");
            assert!(message.contains("gpt-5.5-fast"), "{message}");
            assert!(message.contains("do this work inline"), "{message}");
        }
        other => panic!("expected InvalidInput, got {other:?}"),
    }
}

#[test]
fn live_spawn_path_honors_same_family_explicit_model() {
    let store = TempAgentStore::new("same-family-explicit");
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV)
        .set_also(super::labels::AGENT_STORE_ENV, store.path.as_os_str());
    let mut input = sample_agent_input();
    input.model = Some("gemini-3.5-flash".to_string());

    let manifest = execute_agent_with_spawn_and_parent_model(
        input,
        |job| {
            assert_eq!(job.manifest.model.as_deref(), Some(latest_google_model()));
            assert_eq!(
                job.manifest.resolved_model.as_deref(),
                Some(latest_google_model())
            );
            Ok(())
        },
        Some("gemini-3.5-pro"),
        None,
    )
    .expect("same-family explicit model should reach spawn");

    assert_eq!(manifest.model.as_deref(), Some(latest_google_model()));
}

#[test]
fn live_spawn_path_env_override_beats_smart_route() {
    let store = TempAgentStore::new("env-beats-smart-route");
    let _guard = EnvGuard::set(AGENT_MODEL_ENV, "forced-agent-model")
        .set_also(super::labels::AGENT_STORE_ENV, store.path.as_os_str());
    let mut input = sample_agent_input();
    input.route_model = Some("gemini-3.5-flash".to_string());

    let manifest = execute_agent_with_spawn_and_parent_model(
        input,
        |job| {
            assert_eq!(job.manifest.model.as_deref(), Some("forced-agent-model"));
            assert_eq!(job.thinking_budget_tokens, None);
            Ok(())
        },
        Some("gpt-5.5-fast"),
        None,
    )
    .expect("env override should win over Smart route");

    assert_eq!(manifest.model.as_deref(), Some("forced-agent-model"));
}

#[test]
fn fast_route_keeps_smart_rate_limit_fallbacks_ahead_of_parent() {
    let store = TempAgentStore::new("rate-limit-fallbacks");
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV)
        .set_also(super::labels::AGENT_STORE_ENV, store.path.as_os_str());
    let mut input = sample_agent_input();
    input.subagent_type = Some("Explore".to_string());
    input.route_role = Some("fast".to_string());
    input.route_model = Some("gemini-3.5-flash".to_string());
    input.route_fallback_models = vec![
        "gpt-5.5-fast".to_string(),
        "claude-opus-4-8".to_string(),
        "gpt-5.5-fast".to_string(),
    ];

    execute_agent_with_spawn_and_parent_model(
        input,
        |job| {
            assert_eq!(job.manifest.model.as_deref(), Some("gemini-3.5-flash"));
            assert_ne!(
                job.route_fallback_models.first().map(String::as_str),
                Some("claude-opus-4-8"),
                "one 429 on a Fast route must not promote straight back to the flagship parent"
            );
            assert_eq!(
                job.route_fallback_models,
                vec!["gpt-5.5-fast".to_string(), "claude-opus-4-8".to_string()],
                "unique smart-router candidates stay ahead of the parent, never repeating the selected model"
            );
            Ok(())
        },
        Some("claude-opus-4-8"),
        None,
    )
    .expect("smart-routed agent should carry fallback candidates to the provider client");
}

#[test]
fn live_spawn_path_skips_premium_fast_fallback_for_gpt56_routes() {
    let store = TempAgentStore::new("gpt56-skips-premium-fast-fallback");
    // Pinned to classic: this test exercises the premium-fast FALLBACK filter
    // for a Large-complexity Sol primary, which only the classic policy admits
    // (architect gates a non-explicit reserved primary at any complexity —
    // covered by `implicit_premium_parent_is_blocked_under_architect...`).
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV)
        .set_also(super::labels::AGENT_STORE_ENV, store.path.as_os_str())
        .set_also("ZO_SMART_POLICY", "classic");
    let mut input = sample_agent_input();
    input.route_model = Some("gpt-5.6-terra".to_string());
    input.route_complexity = Some("large".to_string());
    input.route_fallback_models = vec![
        "gpt-5.5-fast".to_string(),
        "openai/gpt-5.5-fast".to_string(),
        "claude-opus-4-8".to_string(),
        "gpt-5.5-fast".to_string(),
    ];

    execute_agent_with_spawn_and_parent_model(
        input,
        |job| {
            assert_eq!(job.manifest.model.as_deref(), Some("gpt-5.6-terra"));
            assert_eq!(
                job.route_fallback_models,
                Vec::<String>::new(),
                "GPT-5.6 Sol/Terra/Luna must not rate-limit fallback into premium gpt-5.5-fast"
            );
            Ok(())
        },
        Some("openai/gpt-5.5-fast"),
        None,
    )
    .expect("gpt-5.6 route should keep non-premium fallback candidates only");
}

#[test]
fn explicit_coding_model_does_not_rate_limit_escalate_to_parent_fable() {
    let store = TempAgentStore::new("explicit-coding-skips-parent-fable");
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV)
        .set_also(super::labels::AGENT_STORE_ENV, store.path.as_os_str());
    let mut input = sample_agent_input();
    input.model = Some("claude-sonnet-5".to_string());
    input.subagent_type = Some("general-purpose".to_string());
    input.description = "workflow phase `implement` item 0".to_string();
    input.prompt = "Implement the scoped fix and run its tests.".to_string();
    input.route_fallback_models = vec![
        "claude-fable-5".to_string(),
        "claude-sonnet-4-6".to_string(),
    ];

    execute_agent_with_spawn_and_parent_model(
        input,
        |job| {
            assert_eq!(job.manifest.requested_model.as_deref(), Some("claude-sonnet-5"));
            assert_eq!(job.manifest.model.as_deref(), Some("claude-sonnet-5"));
            assert_eq!(
                job.route_fallback_models,
                vec!["claude-sonnet-4-6".to_string()],
                "a Sonnet 429 must not turn an ordinary explicit implementation into Fable work"
            );
            Ok(())
        },
        Some("claude-fable-5"),
        None,
    )
    .expect("explicit Sonnet implementation should keep only standard fallbacks");
}

#[test]
fn ordinary_debugger_does_not_use_parent_fable_as_primary_or_429_fallback() {
    let store = TempAgentStore::new("debugger-skips-parent-fable");
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV)
        .set_also(super::labels::AGENT_STORE_ENV, store.path.as_os_str());
    let mut explicit_standard = sample_agent_input();
    explicit_standard.model = Some("claude-sonnet-5".to_string());
    explicit_standard.subagent_type = Some("debugger".to_string());
    explicit_standard.description = "debug a scoped failure".to_string();
    explicit_standard.prompt = "Reproduce the bug, apply the minimal fix, and run tests.".to_string();
    explicit_standard.route_fallback_models = vec![
        "claude-fable-5".to_string(),
        "claude-sonnet-4-6".to_string(),
    ];

    execute_agent_with_spawn_and_parent_model(
        explicit_standard,
        |job| {
            assert_eq!(
                job.route_fallback_models,
                vec!["claude-sonnet-4-6".to_string()],
                "a debugger 429 must not escalate ordinary repair work to Fable"
            );
            Ok(())
        },
        Some("claude-fable-5"),
        None,
    )
    .expect("explicit Sonnet debugger should retain only standard fallbacks");

    let mut implicit = sample_agent_input();
    implicit.subagent_type = Some("debugger".to_string());
    implicit.description = "debug a scoped failure".to_string();
    implicit.prompt = "Reproduce the bug, apply the minimal fix, and run tests.".to_string();
    let error = execute_agent_with_spawn_and_parent_model(
        implicit,
        |_| panic!("ordinary debugger must not inherit the premium parent"),
        Some("claude-fable-5"),
        None,
    )
    .expect_err("ordinary debugger should fail closed without a standard implementer");
    assert!(error.to_string().contains("reserved for orchestration"));
}

#[test]
fn custom_coding_agent_does_not_rate_limit_escalate_to_parent_fable() {
    let store = TempAgentStore::new("custom-coding-skips-parent-fable");
    let definitions = store.path.join("agents");
    std::fs::create_dir_all(&definitions).expect("create custom-agent directory");
    std::fs::write(
        definitions.join("analysis.md"),
        "---\nname: analysis\ndescription: Custom implementation agent\n---\nImplement scoped changes.",
    )
    .expect("write colliding custom agent definition");
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV)
        .set_also(super::labels::AGENT_STORE_ENV, store.path.as_os_str())
        .set_also("ZO_AGENT_DEFS_DIR", definitions.as_os_str());
    let mut input = sample_agent_input();
    input.model = Some("claude-sonnet-5".to_string());
    input.subagent_type = Some("analysis".to_string());
    input.description = "provider ticket 123".to_string();
    input.prompt = "Handle this ticket end-to-end.".to_string();
    input.route_fallback_models = vec![
        "claude-fable-5".to_string(),
        "claude-sonnet-4-6".to_string(),
    ];

    execute_agent_with_spawn_and_parent_model(
        input,
        |job| {
            assert_eq!(job.manifest.subagent_type.as_deref(), Some("analysis"));
            assert_eq!(
                job.route_fallback_models,
                vec!["claude-sonnet-4-6".to_string()],
                "custom definition intent must survive a vague ticket prompt and role-name collision"
            );
            Ok(())
        },
        Some("claude-fable-5"),
        None,
    )
    .expect("custom Sonnet implementation should keep only standard fallbacks");
}

#[test]
fn implicit_premium_parent_is_blocked_for_ordinary_coding_but_exact_model_is_allowed() {
    let store = TempAgentStore::new("implicit-premium-parent-blocked");
    // Pinned to classic so the ("auto", "large") arm below keeps exercising
    // the classic Large-complexity allowance; the architect policy's stricter
    // gate has its own test (`implicit_premium_parent_is_blocked_under_architect...`).
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV)
        .set_also(super::labels::AGENT_STORE_ENV, store.path.as_os_str())
        .set_also("ZO_SMART_POLICY", "classic");
    let mut implicit = sample_agent_input();
    implicit.description = "implement a scoped fix".to_string();
    implicit.prompt = "Implement the change and run its focused test.".to_string();

    let error = execute_agent_with_spawn_and_parent_model(
        implicit,
        |_| panic!("an ineligible implicit premium model must not spawn"),
        Some("claude-fable-5"),
        None,
    )
    .expect_err("ordinary implementation must not fail open to the premium parent");
    assert!(error.to_string().contains("reserved for orchestration"));

    let mut explicit = sample_agent_input();
    explicit.description = "implement a scoped fix".to_string();
    explicit.prompt = "Implement the change and run its focused test.".to_string();
    explicit.model = Some("claude-fable-5".to_string());
    explicit.route_source = Some("pin".to_string());
    execute_agent_with_spawn_and_parent_model(
        explicit,
        |_| Ok(()),
        Some("claude-fable-5"),
        None,
    )
    .expect("an exact user-selected model remains an explicit override");

    for (source, complexity) in [("pin", "medium"), ("pin", "large")] {
        let mut policy_allowed = sample_agent_input();
        policy_allowed.description = "implement a scoped fix".to_string();
        policy_allowed.prompt = "Implement the change and run its focused test.".to_string();
        policy_allowed.route_role = Some("coding".to_string());
        policy_allowed.route_source = Some(source.to_string());
        policy_allowed.route_complexity = Some(complexity.to_string());
        execute_agent_with_spawn_and_parent_model(
            policy_allowed,
            |job| {
                assert_eq!(job.manifest.model.as_deref(), Some("claude-fable-5"));
                Ok(())
            },
            Some("claude-fable-5"),
            None,
        )
        .unwrap_or_else(|error| {
            panic!("{source}/{complexity} is an explicit policy allowance: {error}")
        });
    }
}

#[test]
fn implicit_premium_parent_is_blocked_under_architect_even_at_large_complexity() {
    let store = TempAgentStore::new("implicit-premium-parent-architect");
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV)
        .set_also(super::labels::AGENT_STORE_ENV, store.path.as_os_str())
        .set_also("ZO_SMART_POLICY", "architect");

    // Architect: the Large-complexity escape is gone — an auto route may not
    // hand implementation to the reserved parent no matter how the classifier
    // graded the task.
    let mut large_auto = sample_agent_input();
    large_auto.description = "implement a scoped fix".to_string();
    large_auto.prompt = "Implement the change and run its focused test.".to_string();
    large_auto.route_role = Some("coding".to_string());
    large_auto.route_source = Some("auto".to_string());
    large_auto.route_complexity = Some("large".to_string());
    let error = execute_agent_with_spawn_and_parent_model(
        large_auto,
        |_| panic!("architect must not admit a reserved model to auto implementation"),
        Some("claude-fable-5"),
        None,
    )
    .expect_err("architect blocks Large-complexity implicit premium implementation");
    assert!(error.to_string().contains("reserved for orchestration"));

    // Failure counts are evidence, not authority to spend a reserved model.
    let mut escalated = sample_agent_input();
    escalated.route_role = Some("coding".to_string());
    escalated.route_source = Some("auto".to_string());
    escalated.prior_failures = 2;
    execute_agent_with_spawn_and_parent_model(
        escalated, |_| panic!("failure count cannot authorize a reserved model"), Some("claude-fable-5"), None,
    ).expect_err("a person pin is still required after failures");

    // An exact user-selected model remains an explicit override under architect.
    let mut explicit = sample_agent_input();
    explicit.description = "implement a scoped fix".to_string();
    explicit.prompt = "Implement the change and run its focused test.".to_string();
    explicit.model = Some("claude-fable-5".to_string());
    explicit.route_source = Some("pin".to_string());
    execute_agent_with_spawn_and_parent_model(explicit, |_| Ok(()), Some("claude-fable-5"), None)
        .expect("an exact user-selected model stays allowed under architect");
}

#[test]
fn coding_rate_limit_fallback_never_authorizes_reserved_or_cross_provider_models() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);
    let routed = vec!["claude-fable-5".to_string(), "gpt-5.5".to_string()];

    for policy in [runtime::SmartPolicy::Classic, runtime::SmartPolicy::Architect] {
        let ordinary = rate_limit_fallback_models(
            "gpt-5.6-terra",
            Some("gpt-5.6-sol"),
            &routed,
            true,
            Some("medium"),
            0,
            policy,
        );
        assert_eq!(ordinary, vec!["gpt-5.5".to_string()], "{policy:?}");
    }

    // Neither complexity nor failure counts authorize a different provider or
    // a reserved implementation model.
    for (complexity, prior_failures, policy) in [
        (Some("large"), 0, runtime::SmartPolicy::Classic),
        (Some("medium"), 2, runtime::SmartPolicy::Classic),
        (Some("medium"), 2, runtime::SmartPolicy::Architect),
        (Some("large"), 2, runtime::SmartPolicy::Architect),
    ] {
        assert_eq!(
            rate_limit_fallback_models(
                "gpt-5.6-terra",
                Some("gpt-5.6-sol"),
                &routed,
                true,
                complexity,
                prior_failures,
                policy,
            ),
            vec!["gpt-5.5".to_string()],
            "routed candidates must stay ahead of the eligible parent: \
             {complexity:?}/{prior_failures}/{policy:?}"
        );
    }

    // Architect + Large + no failures: the complexity escape is gone, so a
    // 429 cannot promote implementation into the reserved pool.
    assert_eq!(
        rate_limit_fallback_models(
            "gpt-5.6-terra",
            Some("gpt-5.6-sol"),
            &routed,
            true,
            Some("large"),
            0,
            runtime::SmartPolicy::Architect,
        ),
        vec!["gpt-5.5".to_string()],
        "architect must not unlock premium fallbacks on complexity alone"
    );
}

#[test]
fn rate_limit_fallback_filter_normalizes_openai_qualified_gpt56_models() {
    assert!(super::suppress_cross_family_premium_fast_fallback(
        "openai/gpt-5.6-terra",
        "openai/gpt-5.5-fast"
    ));
    assert!(super::suppress_cross_family_premium_fast_fallback(
        "gpt-5.6-luna-2026-07-09",
        "gpt-5.5-fast"
    ));
    assert!(!super::suppress_cross_family_premium_fast_fallback(
        "gpt-5.5",
        "openai/gpt-5.5-fast"
    ));
    assert!(!super::suppress_cross_family_premium_fast_fallback(
        "custom/gpt-5.6-sol",
        "openai/gpt-5.5-fast"
    ));
}

#[test]
fn live_spawn_path_keeps_legacy_gpt55_fast_rate_limit_fallback() {
    let store = TempAgentStore::new("gpt55-keeps-fast-fallback");
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV)
        .set_also(super::labels::AGENT_STORE_ENV, store.path.as_os_str());
    let mut input = sample_agent_input();
    input.route_model = Some("gpt-5.5".to_string());
    input.route_fallback_models = vec!["gpt-5.5-fast".to_string()];

    execute_agent_with_spawn_and_parent_model(
        input,
        |job| {
            assert_eq!(job.manifest.model.as_deref(), Some("gpt-5.5"));
            assert_eq!(
                job.route_fallback_models,
                vec!["gpt-5.5-fast".to_string()],
                "legacy GPT-5.5 non-fast may still fallback to its explicit fast alias"
            );
            Ok(())
        },
        None,
        None,
    )
    .expect("legacy gpt-5.5 route should keep gpt-5.5-fast fallback");
}

#[test]
fn live_spawn_path_env_override_suppresses_rate_limit_fallback_escape() {
    let store = TempAgentStore::new("env-suppresses-fallbacks");
    let _guard = EnvGuard::set(AGENT_MODEL_ENV, "forced-agent-model")
        .set_also(super::labels::AGENT_STORE_ENV, store.path.as_os_str());
    let mut input = sample_agent_input();
    input.route_model = Some("gemini-3.5-flash".to_string());
    input.route_fallback_models = vec!["gpt-5.5-fast".to_string(), "claude-opus-4-8".to_string()];

    execute_agent_with_spawn_and_parent_model(
        input,
        |job| {
            assert_eq!(job.manifest.model.as_deref(), Some("forced-agent-model"));
            assert!(
                job.route_fallback_models.is_empty(),
                "ZO_AGENT_MODEL is explicit user intent and must not escape to parent/router candidates under quota pressure"
            );
            Ok(())
        },
        Some("gpt-5.5-fast"),
        None,
    )
    .expect("env override should still spawn, without fallback candidates");
}

#[test]
fn live_resolver_honors_person_custom_cross_provider_implementation_pin() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);
    let selection = try_resolve_agent_model_selection(None, false, Some("claude-3-5-haiku"),
        "general-purpose", Some("gpt-5.5-fast"), "Implement a task", "Write the fix.").unwrap();
    assert!(selection.model.contains("haiku"), "{}", selection.model);
}

#[test]
fn explicit_per_agent_model_applies_only_within_provider_family() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);
    // Same family: the model picked a specific Claude model for this subtask
    // (BUG-D5) — honor its registered canonical rather than overriding it with
    // difficulty tiering.
    let same = resolve_agent_model_selection(
        Some("haiku"),
        None,
        "debugger",
        Some("claude-opus-4-8"),
        "Debug a hard bug",
        "Find the root cause.",
    );
    assert_eq!(same.model, "claude-haiku-4-5-20251001");
    assert_eq!(same.thinking_budget_tokens, None);

    let google_same = resolve_agent_model_selection(
        Some("gemini-3.5-flash"),
        None,
        "general-purpose",
        Some("gemini-3.5-pro"),
        "do a task",
        "do a task",
    );
    assert_eq!(google_same.model, latest_google_model());

    let xai_same = resolve_agent_model_selection(
        Some("grok-4-fast"),
        None,
        "general-purpose",
        Some("grok-4"),
        "do a task",
        "do a task",
    );
    assert_eq!(
        xai_same.model,
        api::latest_model_for_provider(api::ProviderKind::Xai)
            .expect("provider catalog must define the current xAI head")
    );

    // Cross family: an on-wire SpawnMultiAgent `agents[].model` must not let a
    // tool call silently switch providers away from the active parent/session.
    let cross = resolve_agent_model_selection(
        Some("claude-3-5-haiku"),
        None,
        "general-purpose",
        Some("gpt-5.5"),
        "do a task",
        "do a task",
    );
    assert_ne!(cross.model, "claude-3-5-haiku");
    assert!(
        cross.model.starts_with("gpt"),
        "cross-family per-agent model falls back to the GPT parent: {}",
        cross.model
    );
}

#[test]
fn custom_frontmatter_model_is_a_person_selected_implementation_exception() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);
    // Same family: a custom Claude agent under a Claude parent uses its model
    // (BUG-R17 — previously parsed but ignored).
    let same = resolve_agent_model_selection(
        None,
        Some("claude-opus-4.6"),
        "general-purpose",
        Some("claude-opus-5"),
        "do a task",
        "do a task",
    );
    // The unregistered 4.6 snaps to the family's registered head (Opus 5).
    assert_eq!(same.model, "claude-opus-5");
    // Cross family: a Claude frontmatter model under a GPT parent is ignored,
    // falling back to inheriting the GPT parent instead of dialing the wrong
    // provider.
    let cross = resolve_agent_model_selection(
        None,
        Some("claude-3-5-haiku"),
        "general-purpose",
        Some("gpt-5.5"),
        "do a task",
        "do a task",
    );
    assert_ne!(cross.model, "claude-3-5-haiku");
    assert!(
        cross.model.starts_with("claude"),
        "person-authored custom model overrides the GPT parent: {}",
        cross.model
    );
}

#[test]
fn infers_and_normalizes_expanded_roster() {
    // Inference routes distinctive task text to the new specialists.
    assert_eq!(
        resolve_subagent_type(None, "Code review this PR", "review the diff for bugs"),
        "code-reviewer"
    );
    assert_eq!(
        resolve_subagent_type(
            None,
            "Debug the panic",
            "find the root cause and fix the bug"
        ),
        "debugger"
    );
    assert_eq!(
        resolve_subagent_type(
            None,
            "Refactor the parser",
            "restructure without behavior change"
        ),
        "refactor"
    );
    assert_eq!(
        resolve_subagent_type(None, "Deep research", "thoroughly research how X works"),
        "deep-research"
    );
    assert_eq!(
        resolve_subagent_type(None, "Analyze the logs", "log analysis of the error rates"),
        "data-analyst"
    );
    // Aliases canonicalize to the registered type names.
    assert_eq!(normalize_subagent_type(Some("reviewer")), "code-reviewer");
    assert_eq!(normalize_subagent_type(Some("debug")), "debugger");
    assert_eq!(normalize_subagent_type(Some("research")), "deep-research");
    assert_eq!(normalize_subagent_type(Some("analyst")), "data-analyst");
    assert_eq!(normalize_subagent_type(Some("refactoring")), "refactor");
    // A plain debug task no longer falls through to Verification.
    assert_ne!(
        resolve_subagent_type(None, "debug why it crashes", ""),
        "Verification"
    );
}

#[test]
fn debugger_toolset_includes_the_debug_mode_pair() {
    // The debugger is the only role granted debug-mode tooling: InstrumentLog
    // for throwaway probes and DebugHypothesis for tracking root-cause guesses.
    let debugger = allowed_tools_for_subagent("debugger");
    assert!(debugger.contains("InstrumentLog"));
    assert!(debugger.contains("DebugHypothesis"));

    // No other role gets DebugHypothesis — it is debugger-only, like InstrumentLog.
    for role in ["general-purpose", "Explore", "code-reviewer", "refactor"] {
        assert!(
            !allowed_tools_for_subagent(role).contains("DebugHypothesis"),
            "{role} must not be granted DebugHypothesis"
        );
    }
}

#[test]
fn infers_harness_type_from_task_text() {
    assert_eq!(
        resolve_subagent_type(None, "Run the test suite", "cargo test the runtime crate"),
        "Verification"
    );
    assert_eq!(
        resolve_subagent_type(None, "", "verify the patch"),
        "Verification"
    );
    assert_eq!(
        resolve_subagent_type(None, "", "verify the fix"),
        "Verification"
    );
    assert_eq!(
        resolve_subagent_type(None, "", "verify the fix this commit made"),
        "Verification"
    );
    assert_eq!(
        resolve_subagent_type(None, "", "run tests for the fix"),
        "Verification"
    );
    assert_eq!(
        resolve_subagent_type(None, "", "run tests for the fix the branch contains"),
        "Verification"
    );
    assert_eq!(
        resolve_subagent_type(None, "", "verify and fix the regression"),
        "general-purpose"
    );
    assert_eq!(
        resolve_subagent_type(None, "", "수정 내용 검증"),
        "Verification"
    );
    assert_eq!(
        resolve_subagent_type(None, "", "수정 내용 리뷰"),
        "code-reviewer"
    );
    assert_eq!(
        resolve_subagent_type(
            None,
            "",
            "플랜설정후 적대적검증 하고 1번부터 5번까지 완벽하게 수정"
        ),
        "general-purpose"
    );
    assert_eq!(
        resolve_subagent_type(
            None,
            "Design the cache layer",
            "What approach should we take?"
        ),
        "Plan"
    );
    assert_eq!(
        resolve_subagent_type(
            None,
            "Find where auth is handled",
            "Locate the login handler"
        ),
        "Explore"
    );
    assert_eq!(
        resolve_subagent_type(Some(""), "say hello", "just reply"),
        "general-purpose"
    );
    // An explicit type always wins over inference and is canonicalized.
    assert_eq!(
        resolve_subagent_type(Some("explorer"), "run tests", "verify everything"),
        "Explore"
    );
}

#[test]
fn builtin_harness_instructions_are_role_specific() {
    assert!(builtin_harness_instruction("Explore").contains("read-only"));
    assert!(builtin_harness_instruction("Plan").contains("architect"));
    assert!(builtin_harness_instruction("Verification").contains("pass/fail"));
    assert!(builtin_harness_instruction("general-purpose").contains("general-purpose"));
    // Every harness keeps the shared delegated-task trailer.
    assert!(builtin_harness_instruction("Explore").contains("delegated task"));
    assert!(builtin_harness_instruction("Explore").contains("Stop condition"));
    assert!(builtin_harness_instruction("Plan").contains("do not try to exhaust the repository"));
    assert!(builtin_harness_instruction("Verification").contains("report the blocker"));
}

#[test]
fn custom_agent_tools_override_resolved_allowlist() {
    let custom = CustomAgent {
        name: "triage".to_string(),
        description: "triage".to_string(),
        tools: Some(vec!["read_file".to_string(), "grep_search".to_string()]),
        model: None,
        system_prompt: "body".to_string(),
        permission: None,
        permission_mode: None,
    };
    let tools = allowed_tools_for_resolved("triage", Some(&custom));
    assert_eq!(tools.len(), 2);
    assert!(tools.contains("read_file") && tools.contains("grep_search"));

    // A body-only custom agent (no declared tools) inherits general-purpose.
    let body_only = CustomAgent {
        tools: None,
        ..custom.clone()
    };
    assert_eq!(
        allowed_tools_for_resolved("triage", Some(&body_only)),
        allowed_tools_for_subagent("general-purpose")
    );

    // Built-in types ignore any custom and keep their static set.
    assert_eq!(
        allowed_tools_for_resolved("Explore", None),
        allowed_tools_for_subagent("Explore")
    );
}

#[test]
fn explicit_custom_agent_wins_before_alias_normalization() {
    let dir = std::env::temp_dir().join(format!(
        "zo-agent-alias-custom-test-{}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("mkdir");
    std::fs::write(
        dir.join("reviewer.md"),
        "---\nname: reviewer\ndescription: Custom reviewer\ntools: read_file\n---\nReview locally.",
    )
    .expect("write custom agent");
    let _guard = EnvGuard::set("ZO_AGENT_DEFS_DIR", dir.to_string_lossy().as_ref());

    let (resolved, custom) = resolve_subagent_type_and_custom_agent(
        Some("reviewer"),
        "review code",
        "review this patch",
    );

    assert_eq!(resolved, "reviewer");
    let custom = custom.expect("reviewer.md should be loaded before aliasing");
    assert_eq!(custom.name, "reviewer");
    assert_eq!(custom.description, "Custom reviewer");

    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn subagent_hook_context_includes_status_and_agent_metadata() {
    let manifest = AgentOutput {
        route_probe_confidence: None,
        agent_id: "agent-1".to_string(),
        parent_session_id: None,
        tool_call_id: None,
        name: "analysis".to_string(),
        label: Some("Analysis".to_string()),
        description: "Analyze project".to_string(),
        subagent_type: Some("Explore".to_string()),
        requested_model: None,
        resolved_model: Some("gpt-5.5".to_string()),
        route_reason: None,
        route_role: None,
        route_complexity: None,
        route_risk: None,
        route_source: None,
        model: Some("gpt-5.5".to_string()),
        sees: Vec::new(),
        status: "running".to_string(),
        output_file: "/tmp/agent-1.md".to_string(),
        manifest_file: "/tmp/agent-1.json".to_string(),
        created_at: "1".to_string(),
        pane: None,
        owner_pid: None,
        run_generation: 0,
        started_at: Some("1".to_string()),
        completed_at: None,
        completion_published_at: None,
        lane_events: Vec::new(),
        current_blocker: None,
        error: None,
        token_history: Vec::new(),
        current_tool: None,
        awaiting_provider_since: None,
        recent_tools: Vec::new(),
        tool_calls: 0,
        current_phase: None,
        output_tail: String::new(),
        last_activity_at: None,
        activity: AgentActivityTelemetry::default(),
        lifecycle: crate::misc_tools::agent_tools::AgentLifecycle::default(),
    };
    let job = AgentJob {
        manifest,
        registry: None,
        prompt: "prompt".to_string(),
        system_prompt: Vec::new(),
        allowed_tools: std::collections::BTreeSet::new(),
        permission_rules: None,
        permission_mode: None,
        cwd: Some(std::path::PathBuf::from("/tmp/work")),
        lsp: None,
        schema: None,
        workflow_member: false,
        one_shot: false,
        plan_shape: None,
        route_tax: None,
        parent_attempt: None,
        time_budget: None,
        thinking_budget_tokens: None,
        route_effort: None,
        api_concurrency: None,
        route_fallback_models: Vec::new(),
        mcp_passthrough: None,
        hook_config: runtime::RuntimeHookConfig::default(),
        cancel_signal: runtime::HookAbortSignal::new(),
        judged_agent: None,
        verify_loop: None,
        parent_model: None,
        steering: runtime::SteeringQueue::default(),
        transcript_path: None,
        resume: false,
    };

    let context = subagent_hook_context(&job, "completed", Some("done"), None);

    assert_eq!(context["status"], "completed");
    assert_eq!(context["agent"]["id"], "agent-1");
    assert_eq!(context["agent"]["name"], "analysis");
    assert_eq!(context["agent"]["cwd"], "/tmp/work");
    assert_eq!(context["result"], "done");
}

#[test]
fn parses_agent_api_concurrency_limit_with_safe_bounds() {
    assert_eq!(parse_agent_api_concurrency_limit(None), None);
    assert_eq!(parse_agent_api_concurrency_limit(Some("")), None);
    assert_eq!(parse_agent_api_concurrency_limit(Some("0")), None);
    assert_eq!(parse_agent_api_concurrency_limit(Some("1")), Some(1));
    assert_eq!(parse_agent_api_concurrency_limit(Some("4")), Some(4));
    assert_eq!(
        parse_agent_api_concurrency_limit(Some("99")),
        Some(MAX_AGENT_MAX_CONCURRENCY)
    );
}

#[test]
fn agent_output_current_tool_roundtrips() {
    let json = r#"{"agentId":"a","name":"n","description":"d","status":"running","outputFile":"o","manifestFile":"m","createdAt":"c","currentTool":"edit_file"}"#;
    let parsed: AgentOutput = serde_json::from_str(json).expect("parse manifest");
    assert_eq!(parsed.current_tool.as_deref(), Some("edit_file"));
    assert_eq!(parsed.requested_model, None);
    assert_eq!(parsed.resolved_model, None);
    let back = serde_json::to_string(&parsed).expect("serialize");
    assert!(back.contains(r#""currentTool":"edit_file""#));
}

#[test]
fn record_current_tool_stamps_manifest() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zo-agent-currenttool-test-{unique}"));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let path = dir.join("a.json");
    let output = dir.join("a.md");
    std::fs::write(&output, "# Agent\n").expect("write output");
    std::fs::write(
        &path,
        serde_json::json!({
            "agentId": "a",
            "name": "n",
            "description": "d",
            "status": "running",
            "outputFile": output.display().to_string(),
            "manifestFile": path.display().to_string(),
            "createdAt": "c"
        })
        .to_string(),
    )
    .expect("write manifest");

    record_current_tool(&path, "edit_file", r#"{"file_path":"src/lib.rs"}"#);

    let text = std::fs::read_to_string(&path).expect("read back");
    let parsed: AgentOutput = serde_json::from_str(&text).expect("parse back");
    assert_eq!(parsed.current_tool.as_deref(), Some("edit_file"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
#[allow(clippy::too_many_lines)]
fn stop_running_agents_since_closes_only_current_live_manifests() {
    fn manifest(dir: &std::path::Path, id: &str, status: &str, created_at: &str) -> AgentOutput {
        AgentOutput {
            route_probe_confidence: None,
            agent_id: id.to_string(),
            parent_session_id: None,
            tool_call_id: None,
            name: id.to_string(),
            label: None,
            description: "agent".to_string(),
            subagent_type: Some("Explore".to_string()),
            requested_model: None,
            resolved_model: Some("gpt-5.5".to_string()),
            route_reason: None,
            route_role: None,
            route_complexity: None,
            route_risk: None,
            route_source: None,
            model: Some("gpt-5.5".to_string()),
            sees: Vec::new(),
            status: status.to_string(),
            output_file: dir.join(format!("{id}.md")).display().to_string(),
            manifest_file: dir.join(format!("{id}.json")).display().to_string(),
            created_at: created_at.to_string(),
            pane: None,
            owner_pid: None,
            run_generation: 0,
            started_at: Some(created_at.to_string()),
            completed_at: None,
            completion_published_at: None,
            lane_events: vec![runtime::LaneEvent::started(created_at.to_string())],
            current_blocker: None,
            error: None,
            token_history: Vec::new(),
            current_tool: Some("bash".to_string()),
            awaiting_provider_since: None,
            recent_tools: Vec::new(),
            tool_calls: 1,
            current_phase: None,
            output_tail: String::new(),
            last_activity_at: None,
            activity: AgentActivityTelemetry::default(),
            lifecycle: crate::misc_tools::agent_tools::AgentLifecycle::default(),
        }
    }
    fn write(manifest: &AgentOutput) {
        std::fs::write(&manifest.output_file, "# Agent\n").expect("write output");
        write_agent_manifest(manifest).expect("write manifest");
    }
    fn status(dir: &std::path::Path, id: &str) -> String {
        let text = std::fs::read_to_string(dir.join(format!("{id}.json"))).expect("read manifest");
        serde_json::from_str::<AgentOutput>(&text)
            .expect("parse manifest")
            .status
    }

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zo-stop-agents-{unique}"));
    std::fs::create_dir_all(&dir).expect("mkdir");
    for item in [
        manifest(&dir, "live-running", "running", "200"),
        manifest(&dir, "live-pending", "pending", "201"),
        manifest(&dir, "old-running", "running", "100"),
        manifest(&dir, "done", "completed", "202"),
    ] {
        write(&item);
    }
    let cancel_signal = runtime::HookAbortSignal::new();
    register_agent_cancel_signal("live-running".to_string(), 0, cancel_signal.clone());

    let stopped = stop_running_agents_in_store_since_without_notify(&dir, 150, "cancelled by test");

    assert_eq!(stopped, 2);
    assert!(
        cancel_signal.is_aborted(),
        "stopping a live manifest should abort the matching in-process agent"
    );
    assert_eq!(status(&dir, "live-running"), "stopped");
    assert_eq!(status(&dir, "live-pending"), "stopped");
    assert_eq!(status(&dir, "old-running"), "running");
    assert_eq!(status(&dir, "done"), "completed");

    let stopped_text =
        std::fs::read_to_string(dir.join("live-running.json")).expect("read stopped manifest");
    let stopped_manifest: AgentOutput =
        serde_json::from_str(&stopped_text).expect("parse stopped manifest");
    assert!(
        stopped_manifest.completed_at.is_some(),
        "stopped agents should become terminal immediately"
    );
    assert_eq!(stopped_manifest.current_tool, None);
    assert!(
        std::fs::read_to_string(dir.join("live-running.md"))
            .expect("read output")
            .contains("- status: stopped"),
        "agent output should record the stop"
    );

    let race = manifest(&dir, "race-running", "running", "203");
    write(&race);
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let handles = (0..2)
        .map(|_| {
            let barrier = barrier.clone();
            let race = race.clone();
            std::thread::spawn(move || {
                barrier.wait();
                persist_agent_stopped_state(&race, "concurrent stop").expect("persist stop")
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let owners = handles
        .into_iter()
        .map(|handle| handle.join().expect("join stop writer"))
        .filter(|transitioned| *transitioned)
        .count();
    assert_eq!(owners, 1, "exactly one racing stop path owns the terminal transition");

    // A late live-activity frame uses the same per-agent lock and must re-read
    // the terminal state rather than resurrect this manifest as `running`.
    record_current_tool(
        std::path::Path::new(&race.manifest_file),
        "read_file",
        r#"{"path":"late.rs"}"#,
    );
    let reread: AgentOutput = serde_json::from_str(
        &std::fs::read_to_string(&race.manifest_file).expect("read raced manifest"),
    )
    .expect("parse raced manifest");
    assert_eq!(reread.status, "stopped");
    assert!(reread.current_tool.is_none());

    let mut outside_output = manifest(&dir, "outside-output", "running", "204");
    let outside_path = std::env::temp_dir().join(format!("zo-agent-outside-{unique}.md"));
    std::fs::write(&outside_path, "outside\n").expect("write outside output");
    outside_output.output_file = outside_path.display().to_string();
    std::fs::write(
        &outside_output.manifest_file,
        serde_json::to_string(&outside_output).expect("serialize outside manifest"),
    )
    .expect("write outside manifest");
    assert!(
        persist_agent_stopped_state(&outside_output, "unsafe output").is_err(),
        "terminal persistence must reject output outside the manifest directory"
    );
    assert_eq!(status(&dir, "outside-output"), "running");
    std::fs::remove_file(outside_path).ok();

    #[cfg(unix)]
    {
        use std::os::unix::fs::symlink;
        let link = manifest(&dir, "symlink-output", "running", "205");
        let target = dir.join("symlink-target.md");
        std::fs::write(&target, "target\n").expect("write link target");
        symlink(&target, &link.output_file).expect("create output symlink");
        std::fs::write(
            &link.manifest_file,
            serde_json::to_string(&link).expect("serialize symlink manifest"),
        )
        .expect("write symlink manifest");
        assert!(
            persist_agent_stopped_state(&link, "unsafe output").is_err(),
            "terminal persistence must reject symlink outputs"
        );
        assert_eq!(status(&dir, "symlink-output"), "running");
    }

    unregister_agent_cancel_signal("live-running", 0);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn external_stop_completion_has_one_owner() {
    assert!(external_stop_owns_completion(true, true));
    assert!(
        !external_stop_owns_completion(true, false),
        "a stop path that lost the terminal transition must not notify"
    );
    assert!(!external_stop_owns_completion(false, true));
}

#[test]
fn zo_process_comm_requires_exact_basename() {
    assert!(is_zo_process_comm("zo"));
    assert!(is_zo_process_comm("/Users/dev/.local/bin/zo"));
    assert!(!is_zo_process_comm("zoom"));
    assert!(!is_zo_process_comm("/usr/local/bin/zoxide"));
    assert!(!is_zo_process_comm("/usr/local/bin/forge"));
}

#[test]
fn orphan_reap_settles_dead_owner_pids_and_spares_live_and_terminal() {
    fn manifest(dir: &std::path::Path, id: &str, status: &str, owner_pid: Option<u32>) -> AgentOutput {
        AgentOutput {
            route_probe_confidence: None,
            agent_id: id.to_string(),
            parent_session_id: None,
            tool_call_id: None,
            name: id.to_string(),
            label: None,
            description: "agent".to_string(),
            subagent_type: Some("Explore".to_string()),
            requested_model: None,
            resolved_model: Some("gpt-5.5".to_string()),
            route_reason: None,
            route_role: None,
            route_complexity: None,
            route_risk: None,
            route_source: None,
            model: Some("gpt-5.5".to_string()),
            sees: Vec::new(),
            status: status.to_string(),
            output_file: dir.join(format!("{id}.md")).display().to_string(),
            manifest_file: dir.join(format!("{id}.json")).display().to_string(),
            created_at: "100".to_string(),
            pane: None,
            owner_pid,
            run_generation: 0,
            started_at: Some("100".to_string()),
            completed_at: None,
            completion_published_at: None,
            lane_events: Vec::new(),
            current_blocker: None,
            error: None,
            token_history: Vec::new(),
            current_tool: None,
            awaiting_provider_since: None,
            recent_tools: Vec::new(),
            tool_calls: 0,
            current_phase: None,
            output_tail: String::new(),
            last_activity_at: None,
            activity: AgentActivityTelemetry::default(),
            lifecycle: crate::misc_tools::agent_tools::AgentLifecycle::default(),
        }
    }
    fn status(dir: &std::path::Path, id: &str) -> String {
        let text = std::fs::read_to_string(dir.join(format!("{id}.json"))).expect("read manifest");
        serde_json::from_str::<AgentOutput>(&text)
            .expect("parse manifest")
            .status
    }

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zo-orphan-reap-{unique}"));
    std::fs::create_dir_all(&dir).expect("mkdir");
    // A PID that cannot be a live process (beyond macOS/Linux pid_max).
    let dead_pid = Some(u32::MAX - 7);
    for item in [
        manifest(&dir, "orphan", "running", dead_pid),
        manifest(&dir, "ours", "running", Some(std::process::id())),
        manifest(&dir, "done", "completed", dead_pid),
        // Legacy unstamped manifest, freshly written — the mtime bound must
        // NOT reap it (only day-old silence qualifies).
        manifest(&dir, "legacy-fresh", "running", None),
    ] {
        std::fs::write(&item.output_file, "# Agent\n").expect("write output");
        write_agent_manifest(&item).expect("write manifest");
    }

    let reaped = reap_orphaned_agents_in_store(&dir);

    assert_eq!(reaped, 1, "only the dead-owner running manifest settles");
    assert_eq!(status(&dir, "orphan"), "stopped");
    assert_eq!(status(&dir, "ours"), "running");
    assert_eq!(status(&dir, "done"), "completed");
    assert_eq!(status(&dir, "legacy-fresh"), "running");
    let text = std::fs::read_to_string(dir.join("orphan.json")).expect("read orphan");
    assert!(text.contains("orphaned: owning process exited"));
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn completion_publication_stamps_the_manifest() {
    let store = TempAgentStore::new("completion-stamp");
    std::fs::create_dir_all(&store.path).expect("create agent store");
    let _guard = EnvGuard::set("ZO_AGENT_STORE", store.path.to_str().expect("utf-8 store"));
    let _completion_guard = completion::lock_completion_store_for_tests();
    let agent_id = format!(
        "agent-completion-stamp-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let manifest_path = store.path.join(format!("{agent_id}.json"));
    let output_path = store.path.join(format!("{agent_id}.md"));
    let manifest = AgentOutput {
        route_probe_confidence: None,
        agent_id: agent_id.clone(),
        parent_session_id: Some("session-parent".to_string()),
        tool_call_id: None,
        name: "stamp probe".to_string(),
        label: None,
        description: "test completion delivery marker".to_string(),
        subagent_type: Some("general-purpose".to_string()),
        requested_model: None,
        resolved_model: None,
        route_reason: None,
        route_role: None,
        route_complexity: None,
        route_risk: None,
        route_source: None,
        model: None,
        sees: Vec::new(),
        status: "completed".to_string(),
        output_file: output_path.display().to_string(),
        manifest_file: manifest_path.display().to_string(),
        created_at: "100".to_string(),
        pane: None,
        owner_pid: Some(std::process::id()),
        run_generation: 1,
        started_at: Some("100".to_string()),
        completed_at: Some("200".to_string()),
        completion_published_at: None,
        lane_events: Vec::new(),
        current_blocker: None,
        error: None,
        token_history: Vec::new(),
        current_tool: None,
        awaiting_provider_since: None,
        recent_tools: Vec::new(),
        tool_calls: 1,
        current_phase: None,
        output_tail: String::new(),
        last_activity_at: Some(200),
        activity: AgentActivityTelemetry::default(),
        lifecycle: crate::misc_tools::agent_tools::AgentLifecycle::default(),
    };
    std::fs::write(&output_path, "# Agent Task\n").expect("write output");
    write_agent_manifest(&manifest).expect("write manifest");
    reset_agent_completion(&agent_id);

    completion::notify_agent_completion(
        AgentCompletion {
            agent_id: agent_id.clone(),
            name: manifest.name.clone(),
            status: "completed".to_string(),
            result: Some("done".to_string()),
            structured: None,
            error: None,
            run: HelperRun::default(),
        },
        Some(1),
        Some(std::path::PathBuf::from(&manifest.manifest_file)),
    );

    let stamped: AgentOutput = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path).expect("read stamped manifest"),
    )
    .expect("parse stamped manifest");
    assert!(
        stamped.completion_published_at.is_some(),
        "publication must leave the delivery marker on the manifest"
    );
    assert_eq!(
        stamped.status, "completed",
        "the stamp must not disturb the terminal state"
    );

    reset_agent_completion(&agent_id);
}

#[test]
fn interactive_channel_send_failure_leaves_delivery_unstamped() {
    let store = TempAgentStore::new("channel-fail-stamp");
    std::fs::create_dir_all(&store.path).expect("create agent store");
    let _guard = EnvGuard::set("ZO_AGENT_STORE", store.path.to_str().expect("utf-8 store"));
    let _completion_guard = completion::lock_completion_store_for_tests();
    let agent_id = format!(
        "agent-channel-fail-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let manifest_path = store.path.join(format!("{agent_id}.json"));
    let output_path = store.path.join(format!("{agent_id}.md"));
    let manifest = AgentOutput {
        route_probe_confidence: None,
        agent_id: agent_id.clone(),
        parent_session_id: Some("session-parent".to_string()),
        tool_call_id: None,
        name: "channel fail probe".to_string(),
        label: None,
        description: "test channel-send failure delivery gating".to_string(),
        subagent_type: Some("general-purpose".to_string()),
        requested_model: None,
        resolved_model: None,
        route_reason: None,
        route_role: None,
        route_complexity: None,
        route_risk: None,
        route_source: None,
        model: None,
        sees: Vec::new(),
        status: "completed".to_string(),
        output_file: output_path.display().to_string(),
        manifest_file: manifest_path.display().to_string(),
        created_at: "100".to_string(),
        pane: None,
        owner_pid: Some(std::process::id()),
        run_generation: 1,
        started_at: Some("100".to_string()),
        completed_at: Some("200".to_string()),
        completion_published_at: None,
        lane_events: Vec::new(),
        current_blocker: None,
        error: None,
        token_history: Vec::new(),
        current_tool: None,
        awaiting_provider_since: None,
        recent_tools: Vec::new(),
        tool_calls: 1,
        current_phase: None,
        output_tail: String::new(),
        last_activity_at: Some(200),
        activity: AgentActivityTelemetry::default(),
        lifecycle: crate::misc_tools::agent_tools::AgentLifecycle::default(),
    };
    std::fs::write(&output_path, "# Agent Task\n").expect("write output");
    write_agent_manifest(&manifest).expect("write manifest");
    reset_agent_completion(&agent_id);

    // An interactive host registered its channel, but the receiver is gone
    // (REPL torn down): the wakeup send fails, so re-injection into the
    // parent conversation cannot have happened — the delivery marker must
    // stay absent.
    let rx = completion::register_agent_completion_channel();
    drop(rx);
    completion::notify_agent_completion(
        AgentCompletion {
            agent_id: agent_id.clone(),
            name: manifest.name.clone(),
            status: "completed".to_string(),
            result: Some("undelivered".to_string()),
            structured: None,
            error: None,
            run: HelperRun::default(),
        },
        Some(1),
        Some(std::path::PathBuf::from(&manifest.manifest_file)),
    );
    // Restore the channel-less default before releasing the store lock so
    // sibling tests keep the store-edge stamping contract.
    completion::clear_agent_completion_channel_for_tests();

    let reread: AgentOutput = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path).expect("read manifest after failed send"),
    )
    .expect("parse manifest after failed send");
    assert!(
        reread.completion_published_at.is_none(),
        "a failed interactive wakeup send must not leave a delivery marker"
    );

    reset_agent_completion(&agent_id);
}

#[test]
fn stale_generation_publication_does_not_stamp_a_resumed_manifest() {
    let store = TempAgentStore::new("stale-stamp");
    std::fs::create_dir_all(&store.path).expect("create agent store");
    let _guard = EnvGuard::set("ZO_AGENT_STORE", store.path.to_str().expect("utf-8 store"));
    let _completion_guard = completion::lock_completion_store_for_tests();
    let agent_id = format!(
        "agent-stale-stamp-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let manifest_path = store.path.join(format!("{agent_id}.json"));
    let output_path = store.path.join(format!("{agent_id}.md"));
    // The on-disk manifest is already the RESUMED generation 2, running —
    // exactly the state a SendMessage resume leaves while generation 1's
    // delayed publication is still in flight.
    let manifest = AgentOutput {
        route_probe_confidence: None,
        agent_id: agent_id.clone(),
        parent_session_id: Some("session-parent".to_string()),
        tool_call_id: None,
        name: "stale stamp probe".to_string(),
        label: None,
        description: "test stale-generation stamp refusal".to_string(),
        subagent_type: Some("general-purpose".to_string()),
        requested_model: None,
        resolved_model: None,
        route_reason: None,
        route_role: None,
        route_complexity: None,
        route_risk: None,
        route_source: None,
        model: None,
        sees: Vec::new(),
        status: "running".to_string(),
        output_file: output_path.display().to_string(),
        manifest_file: manifest_path.display().to_string(),
        created_at: "100".to_string(),
        pane: None,
        owner_pid: Some(std::process::id()),
        run_generation: 2,
        started_at: Some("300".to_string()),
        completed_at: None,
        completion_published_at: None,
        lane_events: Vec::new(),
        current_blocker: None,
        error: None,
        token_history: Vec::new(),
        current_tool: None,
        awaiting_provider_since: None,
        recent_tools: Vec::new(),
        tool_calls: 0,
        current_phase: None,
        output_tail: String::new(),
        last_activity_at: Some(300),
        activity: AgentActivityTelemetry::default(),
        lifecycle: crate::misc_tools::agent_tools::AgentLifecycle::default(),
    };
    std::fs::write(&output_path, "# Agent Task\n").expect("write output");
    write_agent_manifest(&manifest).expect("write manifest");
    reset_agent_completion(&agent_id);

    completion::notify_agent_completion(
        AgentCompletion {
            agent_id: agent_id.clone(),
            name: manifest.name.clone(),
            status: "completed".to_string(),
            result: Some("stale generation result".to_string()),
            structured: None,
            error: None,
            run: HelperRun::default(),
        },
        Some(1),
        Some(std::path::PathBuf::from(&manifest.manifest_file)),
    );

    let reread: AgentOutput = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path).expect("read manifest after stale publication"),
    )
    .expect("parse manifest after stale publication");
    assert!(
        reread.completion_published_at.is_none(),
        "a stale generation's publication must not stamp the resumed run"
    );
    assert_eq!(
        reread.status, "running",
        "the resumed run must stay untouched"
    );

    reset_agent_completion(&agent_id);
}

#[test]
#[allow(clippy::too_many_lines)]
fn dead_worker_running_manifest_is_failed_and_publishes_completion() {
    let store = TempAgentStore::new("dead-worker-reconcile");
    std::fs::create_dir_all(&store.path).expect("create agent store");
    let _guard = EnvGuard::set("ZO_AGENT_STORE", store.path.to_str().expect("utf-8 store"));
    let _completion_guard = completion::lock_completion_store_for_tests();
    let agent_id = format!(
        "agent-dead-worker-{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let manifest_path = store.path.join(format!("{agent_id}.json"));
    let output_path = store.path.join(format!("{agent_id}.md"));
    let manifest = AgentOutput {
        route_probe_confidence: None,
        agent_id: agent_id.clone(),
        parent_session_id: Some("session-parent".to_string()),
        tool_call_id: None,
        name: "dead worker".to_string(),
        label: None,
        description: "test inverse reaper".to_string(),
        subagent_type: Some("general-purpose".to_string()),
        requested_model: None,
        resolved_model: None,
        route_reason: None,
        route_role: None,
        route_complexity: None,
        route_risk: None,
        route_source: None,
        model: None,
        sees: Vec::new(),
        status: "running".to_string(),
        output_file: output_path.display().to_string(),
        manifest_file: manifest_path.display().to_string(),
        created_at: "100".to_string(),
        pane: None,
        owner_pid: Some(std::process::id()),
        run_generation: 7,
        started_at: Some("100".to_string()),
        completed_at: None,
        completion_published_at: None,
        lane_events: vec![runtime::LaneEvent::started("100")],
        current_blocker: None,
        error: None,
        token_history: Vec::new(),
        current_tool: Some("cargo test".to_string()),
        awaiting_provider_since: None,
        recent_tools: Vec::new(),
        tool_calls: 1,
        current_phase: None,
        output_tail: String::new(),
        last_activity_at: Some(100),
        activity: AgentActivityTelemetry::default(),
        lifecycle: crate::misc_tools::agent_tools::AgentLifecycle::default(),
    };
    std::fs::write(&output_path, "# Agent Task\n").expect("write output");
    let mut transcript = runtime::Session::new();
    transcript
        .push_user_text("original task context")
        .expect("seed transcript");
    transcript
        .save_to_path(store.path.join(format!("{agent_id}.session.jsonl")))
        .expect("persist resume snapshot");
    write_agent_manifest(&manifest).expect("write manifest");
    reset_agent_completion(&agent_id);

    assert!(!agent_worker_is_live(&agent_id));
    assert!(
        reconcile_dead_agent_worker(&AgentRegistry::at_root_for_tests("session-reconcile", &store.path), &agent_id),
        "the first sweep must claim and settle the dead worker"
    );
    assert!(
        !reconcile_dead_agent_worker(&AgentRegistry::at_root_for_tests("session-reconcile", &store.path), &agent_id),
        "a second sweep must not publish a duplicate"
    );

    let settled: AgentOutput = serde_json::from_str(
        &std::fs::read_to_string(&manifest_path).expect("read settled manifest"),
    )
    .expect("parse settled manifest");
    assert_eq!(settled.status, "failed");
    assert_eq!(
        settled.error.as_deref(),
        Some("worker died without delivering a result")
    );
    assert!(settled.completed_at.is_some());
    assert!(settled.current_tool.is_none());

    let completion = wait_for_agent_completions(
        std::slice::from_ref(&agent_id),
        std::time::Duration::ZERO,
    );
    assert_eq!(completion.len(), 1);
    assert_eq!(completion[0].status, "failed");
    assert_eq!(
        completion[0].error.as_deref(),
        Some("worker died without delivering a result")
    );

    reset_agent_completion(&agent_id);
    assert!(
        reconcile_dead_agent_worker(&AgentRegistry::at_root_for_tests("session-reconcile", &store.path), &agent_id),
        "a terminal manifest with no stored completion is representable"
    );
    assert!(
        !reconcile_dead_agent_worker(&AgentRegistry::at_root_for_tests("session-reconcile", &store.path), &agent_id),
        "terminal completion recovery also publishes exactly once"
    );

    let captured: std::sync::Arc<std::sync::Mutex<Option<AgentJob>>> =
        std::sync::Arc::default();
    let capture = std::sync::Arc::clone(&captured);
    let resumed = resume_agent_with_spawn(
        None,
        &settled,
        "continue from the preserved context",
        None,
        None,
        None,
        None,
        false,
        move |job| {
            *capture.lock().expect("capture resumed job") = Some(job);
            Ok(())
        },
    )
    .expect("dead-worker failure remains resumable")
    .manifest;
    let resumed_job = captured
        .lock()
        .expect("capture resumed job")
        .take()
        .expect("resume spawn called");
    assert!(resumed_job.resume);
    assert!(resumed_job.transcript_path.is_some());
    assert_eq!(resumed.status, "running");
    assert_eq!(resumed.run_generation, manifest.run_generation + 1);

    clear_background_agent(&resumed.agent_id);
    unregister_agent_cancel_signal(&resumed.agent_id, resumed.run_generation);
    unregister_agent_steering(&resumed.agent_id, resumed.run_generation);
}

#[test]
fn stop_running_agents_since_for_session_leaves_foreign_session_agents_running() {
    fn manifest(
        dir: &std::path::Path,
        id: &str,
        session_id: &str,
        created_at: &str,
    ) -> AgentOutput {
        AgentOutput {
            route_probe_confidence: None,
            agent_id: id.to_string(),
            parent_session_id: Some(session_id.to_string()),
            tool_call_id: None,
            name: id.to_string(),
            label: None,
            description: "agent".to_string(),
            subagent_type: Some("Explore".to_string()),
            requested_model: None,
            resolved_model: Some("gpt-5.5".to_string()),
            route_reason: None,
            route_role: None,
            route_complexity: None,
            route_risk: None,
            route_source: None,
            model: Some("gpt-5.5".to_string()),
            sees: Vec::new(),
            status: "running".to_string(),
            output_file: dir.join(format!("{id}.md")).display().to_string(),
            manifest_file: dir.join(format!("{id}.json")).display().to_string(),
            created_at: created_at.to_string(),
            pane: None,
            owner_pid: None,
            run_generation: 0,
            started_at: Some(created_at.to_string()),
            completed_at: None,
            completion_published_at: None,
            lane_events: Vec::new(),
            current_blocker: None,
            error: None,
            token_history: Vec::new(),
            current_tool: None,
            awaiting_provider_since: None,
            recent_tools: Vec::new(),
            tool_calls: 0,
            current_phase: None,
            output_tail: String::new(),
            last_activity_at: None,
            activity: AgentActivityTelemetry::default(),
            lifecycle: crate::misc_tools::agent_tools::AgentLifecycle::default(),
        }
    }
    fn write(manifest: &AgentOutput) {
        std::fs::write(&manifest.output_file, "# Agent\n").expect("write output");
        write_agent_manifest(manifest).expect("write manifest");
    }
    fn status(dir: &std::path::Path, id: &str) -> String {
        let text = std::fs::read_to_string(dir.join(format!("{id}.json"))).expect("read manifest");
        serde_json::from_str::<AgentOutput>(&text)
            .expect("parse manifest")
            .status
    }

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zo-stop-agents-session-{unique}"));
    std::fs::create_dir_all(&dir).expect("mkdir");
    write(&manifest(&dir, "current", "session-a", "200"));
    write(&manifest(&dir, "foreign", "session-b", "201"));

    let stopped = stop_running_agents_in_store_since_for_session_without_notify(
        &dir,
        150,
        Some("session-a"),
        "cancelled by test",
    );

    assert_eq!(stopped, 1);
    assert_eq!(status(&dir, "current"), "stopped");
    assert_eq!(status(&dir, "foreign"), "running");
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn parent_session_belongs_covers_the_full_filter_matrix() {
    // The shared core that both the typed `tools` stop paths and the CLI's
    // `serde_json::Value` HUD/detail/workflow filters delegate to. This table
    // pins every branch so neither surface can drift from the rule.
    let cases = [
        // (parent_session_id, session_id, allow_unstamped, expected, why)
        (
            None,
            None,
            false,
            true,
            "no session filter keeps everything",
        ),
        (
            Some("s1"),
            None,
            false,
            true,
            "no session filter keeps a stamped row",
        ),
        (
            Some("  "),
            Some("  "),
            false,
            true,
            "blank caller id behaves like None",
        ),
        (
            Some("s1"),
            Some("s1"),
            false,
            true,
            "exact stamped match is visible",
        ),
        (
            Some("s1"),
            Some("s1"),
            true,
            true,
            "allow_unstamped is irrelevant to a match",
        ),
        (
            Some(" s1 "),
            Some("s1"),
            false,
            true,
            "ids are trimmed before comparing",
        ),
        (
            Some("s2"),
            Some("s1"),
            false,
            false,
            "a foreign stamped row is hidden",
        ),
        (
            Some("s2"),
            Some("s1"),
            true,
            false,
            "foreign stays hidden even when unstamped allowed",
        ),
        (
            None,
            Some("s1"),
            false,
            false,
            "unstamped is hidden under a strict caller",
        ),
        (
            Some("   "),
            Some("s1"),
            false,
            false,
            "a blank stamp is treated as unstamped (strict hides)",
        ),
        (
            None,
            Some("s1"),
            true,
            true,
            "unstamped is kept under a back-compat caller",
        ),
    ];
    for (parent, session, allow_unstamped, expected, why) in cases {
        assert_eq!(
            parent_session_belongs(parent, session, allow_unstamped),
            expected,
            "parent={parent:?} session={session:?} allow_unstamped={allow_unstamped}: {why}"
        );
    }
}

#[test]
fn agent_partial_result_salvages_streamed_output_tail() {
    fn manifest(dir: &std::path::Path, id: &str, tail: &str) -> AgentOutput {
        AgentOutput {
            route_probe_confidence: None,
            agent_id: id.to_string(),
            parent_session_id: None,
            tool_call_id: None,
            name: id.to_string(),
            label: None,
            description: "agent".to_string(),
            subagent_type: Some("Explore".to_string()),
            requested_model: None,
            resolved_model: None,
            route_reason: None,
            route_role: None,
            route_complexity: None,
            route_risk: None,
            route_source: None,
            model: None,
            sees: Vec::new(),
            status: "running".to_string(),
            output_file: dir.join(format!("{id}.md")).display().to_string(),
            manifest_file: dir.join(format!("{id}.json")).display().to_string(),
            created_at: "100".to_string(),
            pane: None,
            owner_pid: None,
            run_generation: 0,
            started_at: Some("100".to_string()),
            completed_at: None,
            completion_published_at: None,
            lane_events: Vec::new(),
            current_blocker: None,
            error: None,
            token_history: Vec::new(),
            current_tool: None,
            awaiting_provider_since: None,
            recent_tools: Vec::new(),
            tool_calls: 0,
            current_phase: None,
            output_tail: tail.to_string(),
            last_activity_at: None,
            activity: AgentActivityTelemetry::default(),
            lifecycle: crate::misc_tools::agent_tools::AgentLifecycle::default(),
        }
    }

    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zo-partial-{unique}"));
    std::fs::create_dir_all(&dir).expect("mkdir");

    // A mid-flight agent that has streamed text but not yet completed: its
    // outputTail must be salvageable so a timed-out fan-out still yields work.
    let with_tail = manifest(&dir, "streamed", "partial finding: api.rs:42 leaks a guard");
    write_agent_manifest(&with_tail).expect("write manifest");
    assert_eq!(
        agent_partial_result(&with_tail.manifest_file),
        Some("partial finding: api.rs:42 leaks a guard".to_string()),
    );

    // Nothing streamed yet → no partial result (not an empty string).
    let silent = manifest(&dir, "silent", "");
    write_agent_manifest(&silent).expect("write manifest");
    assert_eq!(agent_partial_result(&silent.manifest_file), None);

    // Whitespace-only tail is treated as empty.
    let blank = manifest(&dir, "blank", "   \n  ");
    write_agent_manifest(&blank).expect("write manifest");
    assert_eq!(agent_partial_result(&blank.manifest_file), None);

    // Missing manifest → None, never a panic.
    assert_eq!(
        agent_partial_result(dir.join("does-not-exist.json").to_str().unwrap()),
        None
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A sub-agent that COMPLETES in the race between the collection window closing
/// and the timeout salvage must not be clobbered to `stopped`, and salvage must
/// return its full (uncapped) result — not the truncated rolling tail.
#[test]
fn cancel_and_salvage_preserves_a_completed_agent_and_returns_full_result() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zo-salvage-completed-{unique}"));
    std::fs::create_dir_all(&dir).expect("mkdir");

    let id = "raced-completed";
    let manifest = AgentOutput {
        route_probe_confidence: None,
        agent_id: id.to_string(),
        parent_session_id: None,
        tool_call_id: None,
        name: id.to_string(),
        label: None,
        description: "agent".to_string(),
        subagent_type: Some("Explore".to_string()),
        requested_model: None,
        resolved_model: None,
        route_reason: None,
        route_role: None,
        route_complexity: None,
        route_risk: None,
        route_source: None,
        model: None,
        sees: Vec::new(),
        status: "completed".to_string(),
        output_file: dir.join(format!("{id}.md")).display().to_string(),
        manifest_file: dir.join(format!("{id}.json")).display().to_string(),
        created_at: "100".to_string(),
        pane: None,
        owner_pid: None,
        run_generation: 0,
        started_at: Some("100".to_string()),
        completed_at: Some("200".to_string()),
        completion_published_at: None,
        lane_events: Vec::new(),
        current_blocker: None,
        error: None,
        token_history: Vec::new(),
        current_tool: None,
        awaiting_provider_since: None,
        recent_tools: Vec::new(),
        tool_calls: 0,
        current_phase: None,
        output_tail: "capped tail".to_string(),
        last_activity_at: None,
        activity: AgentActivityTelemetry::default(),
        lifecycle: crate::misc_tools::agent_tools::AgentLifecycle::default(),
    };
    write_agent_manifest(&manifest).expect("write manifest");
    let full = "# Result\n\nThe complete, uncapped agent report that must survive the race.";
    std::fs::write(&manifest.output_file, full).expect("write output");

    // Salvage returns the FULL result (from the output file), not the tail.
    let salvaged = super::cancel_and_salvage_agent_in(&dir, id, "timed out");
    assert_eq!(salvaged.as_deref(), Some(full.trim()));

    // And it does NOT downgrade the completed manifest to `stopped`.
    let reread: AgentOutput = serde_json::from_str(
        &std::fs::read_to_string(&manifest.manifest_file).expect("reread manifest"),
    )
    .expect("parse manifest");
    assert_eq!(
        reread.status, "completed",
        "a completed manifest must survive a salvage race"
    );

    std::fs::remove_dir_all(&dir).ok();
}

/// A sub-agent that already self-stopped (own time budget, or a cooperative
/// cancel it acknowledged) is terminal `stopped`, but its `output_file` holds
/// only status boilerplate — the real partial work lives only in the streamed
/// `output_tail`. Salvage must return that tail, not the boilerplate. Regression
/// for the terminal short-circuit that returned the (non-empty) boilerplate and
/// silently discarded the partial findings.
#[test]
fn cancel_and_salvage_of_a_stopped_agent_returns_the_streamed_tail_not_boilerplate() {
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("zo-salvage-stopped-{unique}"));
    std::fs::create_dir_all(&dir).expect("mkdir");

    let id = "self-stopped";
    let partial = "PARTIAL FINDINGS: api.rs:42 drops a guard under load";
    let manifest = AgentOutput {
        route_probe_confidence: None,
        agent_id: id.to_string(),
        parent_session_id: None,
        tool_call_id: None,
        name: id.to_string(),
        label: None,
        description: "agent".to_string(),
        subagent_type: Some("Explore".to_string()),
        requested_model: None,
        resolved_model: None,
        route_reason: None,
        route_role: None,
        route_complexity: None,
        route_risk: None,
        route_source: None,
        model: None,
        sees: Vec::new(),
        status: "stopped".to_string(),
        output_file: dir.join(format!("{id}.md")).display().to_string(),
        manifest_file: dir.join(format!("{id}.json")).display().to_string(),
        created_at: "100".to_string(),
        pane: None,
        owner_pid: None,
        run_generation: 0,
        started_at: Some("100".to_string()),
        completed_at: Some("200".to_string()),
        completion_published_at: None,
        lane_events: Vec::new(),
        current_blocker: None,
        error: None,
        token_history: Vec::new(),
        current_tool: None,
        awaiting_provider_since: None,
        recent_tools: Vec::new(),
        tool_calls: 0,
        current_phase: None,
        output_tail: partial.to_string(),
        last_activity_at: None,
        activity: AgentActivityTelemetry::default(),
        lifecycle: crate::misc_tools::agent_tools::AgentLifecycle::default(),
    };
    write_agent_manifest(&manifest).expect("write manifest");
    // A stopped agent's output_file is non-empty boilerplate with no real work.
    std::fs::write(&manifest.output_file, "# Task\n\n## Result\n\n- status: stopped\n")
        .expect("write boilerplate");

    let salvaged = super::cancel_and_salvage_agent_in(&dir, id, "timed out");
    assert_eq!(
        salvaged.as_deref(),
        Some(partial),
        "a stopped agent's salvage must return the streamed tail, not the boilerplate"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn collection_window_timeout_is_not_a_terminal_completion() {
    // Only completed/failed/stopped are real results. A `still_running`
    // placeholder (collection window elapsed) must NOT count as terminal — it is
    // what drives the cancel + partial-salvage path in aggregation.
    assert!(agent_output_status_is_terminal("completed"));
    assert!(agent_output_status_is_terminal("failed"));
    assert!(agent_output_status_is_terminal("stopped"));
    assert!(!agent_output_status_is_terminal("still_running"));
    assert!(!agent_output_status_is_terminal("running"));
    assert!(!agent_output_status_is_terminal("pending"));
}

#[test]
fn route_outcome_record_is_aggregate_safe_completion_metadata() {
    let dir = std::env::temp_dir().join("zo-route-outcome-record-test");
    let manifest = AgentOutput {
        route_probe_confidence: None,
        agent_id: "agent-1".to_string(),
        parent_session_id: Some("session-1".to_string()),
        tool_call_id: None,
        name: "security verifier".to_string(),
        label: None,
        description: "verify auth change".to_string(),
        subagent_type: Some("Verification".to_string()),
        requested_model: Some("auto".to_string()),
        // A self-canonical id (alias == canonical_model_id in the registry)
        // on purpose: `canonicalize_route_model_id` (P3 write-time
        // canonicalization) is a deterministic no-op for it regardless of
        // which providers happen to be enabled/credentialed in the test
        // process — unlike a bare alias (`gpt-5.5`) whose resolution to its
        // dated canonical id depends on ambient OpenAI-configured state.
        resolved_model: Some("gpt-5.5-fast".to_string()),
        route_reason: None,
        route_role: Some("verifier".to_string()),
        route_complexity: Some("medium".to_string()),
        route_risk: Some("low".to_string()),
        route_source: Some("auto".to_string()),
        model: Some("gpt-5.5-fast".to_string()),
        sees: Vec::new(),
        status: "running".to_string(),
        output_file: dir.join("agent-1.md").display().to_string(),
        manifest_file: dir.join("agent-1.json").display().to_string(),
        created_at: "1".to_string(),
        pane: None,
        owner_pid: None,
        run_generation: 0,
        started_at: Some("1".to_string()),
        completed_at: None,
        completion_published_at: None,
        lane_events: Vec::new(),
        current_blocker: None,
        error: None,
        token_history: Vec::new(),
        current_tool: None,
        awaiting_provider_since: None,
        recent_tools: Vec::new(),
        tool_calls: 0,
        current_phase: None,
        output_tail: String::new(),
        last_activity_at: None,
        activity: AgentActivityTelemetry::default(),
        lifecycle: crate::misc_tools::agent_tools::AgentLifecycle::default(),
    };
    let completion = completion::AgentCompletion {
        agent_id: "agent-1".to_string(),
        name: "security verifier".to_string(),
        status: "failed".to_string(),
        result: Some("raw agent output that must not be persisted".to_string()),
        structured: Some(serde_json::json!({ "providerErrorClass": "rateLimit" })),
        error: Some("raw provider error must not be persisted".to_string()),
        run: HelperRun { output_tokens: 42, ..HelperRun::default() },
    };

    let record = spawn::route_outcome_record_for_tests(&manifest, &completion, &manifest);
    let json = serde_json::to_string(&record).expect("serialize record");

    assert_eq!(record.route_key, "subagent:Verification");
    assert_eq!(record.selected_model, "gpt-5.5-fast");
    assert_eq!(record.requested_model.as_deref(), Some("auto"));
    assert_eq!(record.provider_error_class.as_deref(), Some("rateLimit"));
    assert_eq!(record.output_tokens, 42);
    assert_eq!(record.role.as_deref(), Some("verifier"));
    assert_eq!(record.complexity.as_deref(), Some("medium"));
    assert_eq!(record.risk.as_deref(), Some("low"));
    assert_eq!(record.route_source.as_deref(), Some("auto"));
    // A run-level spawn completion is a MODEL decision (which model the router
    // picked), stamped explicitly — not left to the absent==model default.
    assert_eq!(record.decision.as_deref(), Some("model"));
    // The run's attempt identity — the agent id and the generation it ran as —
    // the same one a verdict about this run stamps (t-3920).
    assert_eq!(record.run_id.as_deref(), Some("agent-1#0"));
    // `started_at`/`completed_at` aren't both set on this fixture (`running`,
    // no `completed_at`), so duration stays unknown rather than a guess.
    assert_eq!(record.duration_ms, None);
    assert!(!json.contains("raw agent output"));
    assert!(!json.contains("raw provider error"));

    let mut fallback_manifest = manifest.clone();
    fallback_manifest.subagent_type = Some("   ".to_string());
    fallback_manifest.requested_model = Some("   ".to_string());
    fallback_manifest.resolved_model = None;
    fallback_manifest.model = None;
    let mut fallback_completion = completion.clone();
    fallback_completion.structured = Some(serde_json::json!({ "providerErrorClass": "   " }));

    let fallback =
        spawn::route_outcome_record_for_tests(&fallback_manifest, &fallback_completion, &fallback_manifest);

    assert_eq!(fallback.route_key, "subagent:general-purpose");
    assert_eq!(fallback.selected_model, "unknown");
    assert_eq!(fallback.requested_model, None);
    assert_eq!(fallback.provider_error_class, None);
}

fn recorder_manifest(dir: &std::path::Path, agent_id: &str) -> AgentOutput {
    AgentOutput {
        route_probe_confidence: None,
        agent_id: agent_id.to_string(),
        parent_session_id: None,
        tool_call_id: None,
        name: "worker".to_string(),
        label: None,
        description: "do work".to_string(),
        subagent_type: Some("general-purpose".to_string()),
        requested_model: Some("auto".to_string()),
        // The model this agent STARTED on.
        resolved_model: Some("gpt-5.6-sol".to_string()),
        route_reason: None,
        route_role: Some("coding".to_string()),
        route_complexity: Some("medium".to_string()),
        route_risk: Some("low".to_string()),
        route_source: Some("auto".to_string()),
        model: Some("gpt-5.6-sol".to_string()),
        sees: Vec::new(),
        status: "running".to_string(),
        output_file: dir.join(format!("{agent_id}.md")).display().to_string(),
        manifest_file: dir.join(format!("{agent_id}.json")).display().to_string(),
        created_at: "100".to_string(),
        pane: None,
        owner_pid: None,
        run_generation: 0,
        started_at: Some("100".to_string()),
        completed_at: None,
        completion_published_at: None,
        lane_events: Vec::new(),
        current_blocker: None,
        error: None,
        token_history: Vec::new(),
        current_tool: None,
        awaiting_provider_since: None,
        recent_tools: Vec::new(),
        tool_calls: 0,
        current_phase: None,
        output_tail: String::new(),
        last_activity_at: None,
        activity: AgentActivityTelemetry::default(),
        lifecycle: crate::misc_tools::agent_tools::AgentLifecycle::default(),
    }
}

/// P3 verified misattribution fix: a mid-run rate-limit/starvation swap
/// (`record_agent_runtime_model`) updates ONLY the on-disk manifest file, not
/// the in-memory copy captured at spawn time. Simulates that by writing a
/// manifest to disk with a DIFFERENT `resolvedModel` than the in-memory
/// `spawn_time_manifest`, and asserts the re-read prefers the on-disk value.
#[test]
fn spawn_completion_recorder_prefers_on_disk_resolved_model_over_swap() {
    let dir = std::env::temp_dir().join(format!(
        "zo-manifest-reread-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).expect("temp dir");
    let manifest_path = dir.join("agent-1.json");

    let spawn_time_manifest = recorder_manifest(&dir, "agent-1");

    // Simulate a mid-run starvation/rate-limit swap PLUS the terminal
    // `persist_agent_*_state` write: the on-disk manifest now carries the
    // model the agent actually FINISHED on, and a completed status/timestamp
    // — exactly what `record_agent_runtime_model` + the terminal persist call
    // would have produced, without exercising that whole pipeline here.
    let mut on_disk_manifest = spawn_time_manifest.clone();
    on_disk_manifest.resolved_model = Some("gpt-5.6-luna".to_string());
    on_disk_manifest.model = Some("gpt-5.6-luna".to_string());
    on_disk_manifest.status = "completed".to_string();
    on_disk_manifest.completed_at = Some("142".to_string());
    std::fs::write(
        &manifest_path,
        serde_json::to_string(&on_disk_manifest).expect("serialize on-disk manifest"),
    )
    .expect("write on-disk manifest");

    let merged = spawn::current_on_disk_manifest_or_spawn_time_for_tests(&spawn_time_manifest);
    assert_eq!(
        merged.resolved_model.as_deref(),
        Some("gpt-5.6-luna"),
        "must credit the model the agent swapped TO, not the spawn-time model"
    );
    assert_eq!(merged.completed_at.as_deref(), Some("142"));

    // And the outcome record built from the merged manifest reflects the
    // swapped-to model, canonicalized.
    let completion = completion::AgentCompletion {
        agent_id: "agent-1".to_string(),
        name: "worker".to_string(),
        status: "completed".to_string(),
        result: Some("done".to_string()),
        structured: None,
        error: None,
        run: HelperRun { output_tokens: 10, ..HelperRun::default() },
    };
    let record = spawn::route_outcome_record_for_tests(&merged, &completion, &spawn_time_manifest);
    assert_eq!(record.selected_model, "gpt-5.6-luna");
    assert_eq!(record.duration_ms, Some(42_000));

    // The attempt is the JOB's run, not whatever the file says by now: a
    // resume that already advanced the on-disk generation must not re-label
    // this run's outcome as the next attempt's (t-3920).
    let mut resumed_on_disk = merged.clone();
    resumed_on_disk.run_generation = spawn_time_manifest.run_generation + 1;
    let record = spawn::route_outcome_record_for_tests(&resumed_on_disk, &completion, &spawn_time_manifest);
    assert_eq!(record.run_id.as_deref(), Some("agent-1#0"));

    // Nor may the file lend that next attempt's model or route to this run:
    // once a resume owns it (generation 1, on another model and route), the
    // reread hands back the spawn-time copy, and the outcome of the run that
    // ended names the model and route it ran on (t-3959).
    let mut next_attempt = on_disk_manifest.clone();
    next_attempt.run_generation = spawn_time_manifest.run_generation + 1;
    next_attempt.resolved_model = Some("claude-opus-4-8".to_string());
    next_attempt.model = Some("claude-opus-4-8".to_string());
    next_attempt.route_role = Some("debugging".to_string());
    next_attempt.route_source = Some("pin".to_string());
    next_attempt.status = "running".to_string();
    std::fs::write(
        &manifest_path,
        serde_json::to_string(&next_attempt).expect("serialize resumed manifest"),
    )
    .expect("write resumed manifest");
    let reread = spawn::current_on_disk_manifest_or_spawn_time_for_tests(&spawn_time_manifest);
    assert_eq!(reread.run_generation, 0);
    assert_eq!(reread.resolved_model.as_deref(), Some("gpt-5.6-sol"));
    let record = spawn::route_outcome_record_for_tests(&reread, &completion, &spawn_time_manifest);
    assert_eq!(record.selected_model, "gpt-5.6-sol");
    assert_eq!(record.role.as_deref(), Some("coding"));
    assert_eq!(record.route_source.as_deref(), Some("auto"));
    assert_eq!(record.run_id.as_deref(), Some("agent-1#0"));

    let _ = std::fs::remove_dir_all(&dir);
}

/// Falls back to the spawn-time manifest verbatim when there is no on-disk
/// file to read (e.g. it was already cleaned up, or a test harness that
/// never wrote one) — never a hard failure.
#[test]
fn manifest_reread_falls_back_to_spawn_time_copy_without_a_file() {
    let dir = std::env::temp_dir().join(format!(
        "zo-manifest-reread-missing-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let mut spawn_time_manifest = recorder_manifest(&dir, "agent-2");
    spawn_time_manifest.requested_model = None;
    spawn_time_manifest.route_role = None;
    spawn_time_manifest.route_complexity = None;
    spawn_time_manifest.route_risk = None;
    spawn_time_manifest.route_source = None;

    let merged = spawn::current_on_disk_manifest_or_spawn_time_for_tests(&spawn_time_manifest);
    assert_eq!(merged.resolved_model.as_deref(), Some("gpt-5.6-sol"));
}

/// The spawn clamp: a child never exceeds the spawning session's privilege,
/// while unranked (`Prompt`/`Allow`) or unknown parents keep the legacy
/// requested-or-default behavior byte-identical.
#[test]
fn clamped_spawn_mode_never_exceeds_parent_privilege() {
    use runtime::PermissionMode::{Allow, DangerFullAccess, Prompt, ReadOnly, WorkspaceWrite};

    // Read-only parent: the historical `None → DangerFullAccess` default and
    // an explicit workspace-write request both collapse to read-only.
    assert_eq!(clamped_spawn_mode(Some(ReadOnly), None), Some(ReadOnly));
    assert_eq!(
        clamped_spawn_mode(Some(ReadOnly), Some(WorkspaceWrite)),
        Some(ReadOnly)
    );
    // Workspace-write parent caps at workspace-write, keeps lower requests.
    assert_eq!(
        clamped_spawn_mode(Some(WorkspaceWrite), None),
        Some(WorkspaceWrite)
    );
    assert_eq!(
        clamped_spawn_mode(Some(WorkspaceWrite), Some(ReadOnly)),
        Some(ReadOnly)
    );
    // Danger parent: byte-identical to the pre-clamp behavior.
    assert_eq!(
        clamped_spawn_mode(Some(DangerFullAccess), None),
        Some(DangerFullAccess)
    );
    assert_eq!(
        clamped_spawn_mode(Some(DangerFullAccess), Some(ReadOnly)),
        Some(ReadOnly)
    );
    // Unranked parents gate interactively — no static clamp.
    assert_eq!(
        clamped_spawn_mode(Some(Prompt), None),
        Some(DangerFullAccess)
    );
    assert_eq!(
        clamped_spawn_mode(Some(Allow), Some(WorkspaceWrite)),
        Some(WorkspaceWrite)
    );
    // Unknown parent (headless/test hosts): requested passes through.
    assert_eq!(clamped_spawn_mode(None, Some(WorkspaceWrite)), Some(WorkspaceWrite));
    assert_eq!(clamped_spawn_mode(None, None), None);
}

/// `agent_worker_is_live` mirrors the cancel-signal registry exactly: present
/// between spawn/resume registration and worker teardown, absent after.
#[test]
fn agent_worker_liveness_follows_cancel_signal_registration() {
    let agent_id = "worker-liveness-probe";
    assert!(!agent_worker_is_live(agent_id));
    register_agent_cancel_signal(
        agent_id.to_string(),
        0,
        runtime::HookAbortSignal::new(),
    );
    assert!(agent_worker_is_live(agent_id));
    unregister_agent_cancel_signal(agent_id, 0);
    assert!(!agent_worker_is_live(agent_id));
}

/// A store of one's own for a test that writes manifests by their own path
/// and never touches the process env — the shape `steer.rs` and the harness
/// equivalence tests need. Removed with the value.
pub(super) struct IsolatedStore {
    pub(super) store: PathBuf,
}

impl IsolatedStore {
    pub(super) fn new(name: &str) -> Self {
        let store = std::env::temp_dir().join(format!(
            "zo-agent-store-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        std::fs::create_dir_all(&store).expect("store");
        Self {
            store: std::fs::canonicalize(&store).expect("canonical store"),
        }
    }

    /// A `running` manifest of generation zero, written to disk with its
    /// output file beside it.
    pub(super) fn running_manifest(&self, agent_id: &str) -> AgentOutput {
        let manifest = AgentOutput {
            route_probe_confidence: None,
            agent_id: agent_id.to_string(),
            parent_session_id: Some("session-pane-test".to_string()),
            tool_call_id: None,
            name: agent_id.to_string(),
            label: None,
            description: "agent".to_string(),
            subagent_type: Some("general-purpose".to_string()),
            requested_model: None,
            resolved_model: None,
            route_reason: None,
            route_role: None,
            route_complexity: None,
            route_risk: None,
            route_source: None,
            sees: Vec::new(),
            model: None,
            status: "running".to_string(),
            output_file: self.store.join(format!("{agent_id}.md")).display().to_string(),
            manifest_file: self.store.join(format!("{agent_id}.json")).display().to_string(),
            created_at: "100".to_string(),
            pane: None,
            owner_pid: Some(std::process::id()),
            run_generation: 0,
            started_at: Some("100".to_string()),
            completed_at: None,
            completion_published_at: None,
            lane_events: Vec::new(),
            current_blocker: None,
            error: None,
            token_history: Vec::new(),
            current_tool: None,
            awaiting_provider_since: None,
            recent_tools: Vec::new(),
            tool_calls: 0,
            current_phase: None,
            output_tail: String::new(),
            last_activity_at: None,
            activity: AgentActivityTelemetry::default(),
            lifecycle: crate::misc_tools::agent_tools::AgentLifecycle::default(),
        };
        std::fs::write(&manifest.output_file, "# Agent\n").expect("write output");
        super::manifest::write_agent_manifest(&manifest).expect("write manifest");
        manifest
    }

    pub(super) fn read_manifest(&self, agent_id: &str) -> AgentOutput {
        serde_json::from_str(
            &std::fs::read_to_string(self.store.join(format!("{agent_id}.json"))).expect("manifest"),
        )
        .expect("parse manifest")
    }
}

impl Drop for IsolatedStore {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.store);
    }
}

/// The fan-out's classifier helpers (decomposition, triage) answer one
/// structured question and are watched by nobody: inline even where the
/// launch runs helpers in panes. Every other role keeps the launch's mode.
#[test]
fn a_classifier_helper_runs_inline_even_where_panes_are_the_mode() {
    use runtime::subagent_panes::SubagentMode;
    assert_eq!(
        super::spawn_mode_for(SubagentMode::Panes, Some("classifier")),
        SubagentMode::Inline
    );
    assert_eq!(
        super::spawn_mode_for(SubagentMode::Panes, Some("Explore")),
        SubagentMode::Panes
    );
    assert_eq!(super::spawn_mode_for(SubagentMode::Inline, None), SubagentMode::Inline);
}

#[test]
fn execution_quality_reserved_and_cross_family_models_need_person_authority() {
    let parent = Some("gpt-6-astra");
    for (model, parent) in [("claude-fable-5-1", parent), ("claude-fable-5-1", Some("claude-sonnet-4-6"))] {
        let refusal = implementation_model_refusal(model, parent, false).expect("untrusted pin refused");
        assert!(refusal.contains("Do this work inline"));
        assert!(implementation_model_refusal(model, parent, true).is_none());
    }
    assert!(implementation_model_refusal("gpt-5.6-terra", parent, false).is_none());
}

#[test]
fn execution_quality_person_configured_deep_model_is_honored() {
    let store = TempAgentStore::new("person-deep-model");
    let config = tempfile::tempdir().unwrap();
    std::fs::write(config.path().join("settings.json"), r#"{"smart":{"deepTierModels":["claude-fable-5-1"]}}"#).unwrap();
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV)
        .set_also(super::labels::AGENT_STORE_ENV, store.path.as_os_str())
        .set_also("ZO_CONFIG_HOME", config.path().as_os_str());
    let mut input = sample_agent_input();
    input.model = Some("claude-fable-5-1".to_string());
    input.description = "Implement the copy".to_string();
    input.prompt = "Write the new copy in index.html.".to_string();
    execute_agent_with_spawn_and_parent_model(input, |job| {
        assert_eq!(job.manifest.model.as_deref(), Some("claude-fable-5-1"));
        assert!(job.route_fallback_models.is_empty(), "a person pin has no silent quota substitute");
        Ok(())
    }, Some("gpt-6-astra"), None).unwrap();
}

#[test]
fn execution_quality_qualified_parent_and_absent_parent_are_not_foreign_families() {
    assert!(implementation_model_refusal("gpt-5.6-terra", Some("openai/gpt-5.5-fast"), false).is_none());
    assert!(implementation_model_refusal("gpt-5.5", None, false).is_none());
}

#[test]
fn execution_quality_read_only_harnesses_do_not_become_implementation_from_quoted_work() {
    let _guard = EnvGuard::clear(AGENT_MODEL_ENV);
    let selection = try_resolve_agent_model_selection(Some("claude-fable-5-1"), true, None,
        "code-reviewer", Some("gpt-6-astra"), "Review the change", "Review the change. Do not edit files.");
    assert!(selection.is_ok(), "a requested reviewer keeps its read-only role: {selection:?}");
    assert!(!subagent_profile::implementation_task("classifier", "classify", "Classify this request: implement the fix."));
}
