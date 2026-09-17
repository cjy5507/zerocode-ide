//! Auto worktree-isolation policy for the flat `SpawnMultiAgent` fan-out
//! (tracks 3-3 + 4-3).
//!
//! The workflow engine already isolates parallel editors with
//! `isolation:"worktree"` (see [`crate::workflow_tools::worktree`]). A bare
//! `SpawnMultiAgent` fan-out had no such protection: two agents editing the
//! shared working tree at once silently clobber each other, and a *heterogeneous*
//! fan-out (different providers/models per agent — track 3-3) makes that race
//! more likely the moment it edits. This module decides **when** such a fan-out
//! should be auto-placed in per-agent git worktrees, promoting the engine's
//! isolation to the session-level spawn path (track 4-3).
//!
//! The decision is a pure function ([`should_auto_isolate`]) so it is unit
//! testable without spawning agents or git; the spawn loop consumes its verdict
//! to create a [`crate::workflow_tools::worktree::GitWorktreeProvider`] and
//! inject a per-agent `cwd`, then merges each change-set back after the batch
//! barrier (the same collect-patch → 3-way-apply path the engine uses).
//!
//! It is **opt-in** and conservative: off entirely unless `ZO_WORKSPACE_GUARD`
//! is enabled, never overrides an agent that already has an explicit `cwd`, and
//! degrades to "run without isolation" (never an error) whenever git is
//! unavailable — so the default single-process workflow is unchanged.

use serde_json::Value;

/// Whether a `SpawnMultiAgent` fan-out of `agents` should be auto-isolated into
/// per-agent worktrees, given the opt-in `guard_enabled` flag.
///
/// Isolation is warranted only when **every** condition holds:
/// - the workspace guard is opt-in enabled (`guard_enabled`),
/// - the fan-out has at least two agents (a single agent cannot race itself),
/// - no agent already pins an explicit `cwd` (respect a caller that placed
///   agents deliberately — e.g. the workflow engine, which isolates itself).
///
/// Returns `false` for the common solo/sequential case so the default path is
/// untouched. (Caller still falls back to no isolation if git is unavailable.)
#[must_use]
pub(crate) fn should_auto_isolate(agents: &[Value], guard_enabled: bool) -> bool {
    guard_enabled
        && agents.len() >= 2
        && agents
            .iter()
            .all(|agent| agent.get("cwd").is_none_or(Value::is_null))
}

/// Whether a fan-out is *heterogeneous* — agents will run on two or more
/// distinct models (hence potentially different providers). Track 3-3's
/// motivating case: running Claude + GPT + Gemini agents in parallel. Not
/// required for the isolation decision (any concurrent edit races), but recorded
/// in the run summary so the user can see *why* isolation engaged.
///
/// Buckets by the EFFECTIVE model each agent will run on (see
/// [`effective_agent_model`]): an explicit on-wire `model`, else a resolved
/// Smart-route model, else `None` = inherit the parent. Reading only `model`
/// would miss a cross-provider `/smart` fan-out — routed members keep `model`
/// empty and carry the pick under `__zo_route_model` — under-reporting the
/// very heterogeneity that motivated isolation.
#[must_use]
pub(crate) fn is_heterogeneous(agents: &[Value]) -> bool {
    let mut models = std::collections::BTreeSet::new();
    for agent in agents {
        models.insert(effective_agent_model(agent));
        if models.len() >= 2 {
            return true;
        }
    }
    false
}

/// The model an agent will ACTUALLY run on, for heterogeneity bucketing: an
/// explicit on-wire `model` wins, else the resolved Smart-route model smuggled
/// in by `apply_smart_models_to_spawn_input` (which runs before this on the same
/// input), else `None` = inherit the parent. Whitespace-only values are treated
/// as absent.
fn effective_agent_model(agent: &Value) -> Option<&str> {
    fn field_str<'a>(agent: &'a Value, key: &str) -> Option<&'a str> {
        agent
            .get(key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|model| !model.is_empty())
    }
    field_str(agent, "model")
        .or_else(|| field_str(agent, super::smart_router::ROUTE_MODEL_SMUGGLE_KEY))
}

#[cfg(test)]
mod tests {
    use super::{is_heterogeneous, should_auto_isolate};
    use serde_json::{json, Value};

    #[test]
    fn isolates_a_multi_agent_fan_out_when_guard_enabled() {
        let agents = vec![json!({"prompt": "a"}), json!({"prompt": "b"})];
        assert!(should_auto_isolate(&agents, true));
    }

    #[test]
    fn never_isolates_when_guard_disabled() {
        let agents = vec![json!({"prompt": "a"}), json!({"prompt": "b"})];
        assert!(
            !should_auto_isolate(&agents, false),
            "auto-isolation is strictly opt-in"
        );
    }

    #[test]
    fn never_isolates_a_single_agent() {
        let agents = vec![json!({"prompt": "solo"})];
        assert!(
            !should_auto_isolate(&agents, true),
            "one agent cannot race itself"
        );
    }

    #[test]
    fn respects_an_explicit_per_agent_cwd() {
        // A caller that already placed an agent (e.g. the workflow engine) must
        // not be second-guessed — its `cwd` wins.
        let agents = vec![
            json!({"prompt": "a", "cwd": "/tmp/wt-a"}),
            json!({"prompt": "b"}),
        ];
        assert!(!should_auto_isolate(&agents, true));
    }

    #[test]
    fn heterogeneous_detects_distinct_models() {
        let mixed = vec![
            json!({"prompt": "a", "model": "claude-opus-4-8"}),
            json!({"prompt": "b", "model": "gpt-5.5-fast"}),
        ];
        assert!(is_heterogeneous(&mixed));

        let same = vec![
            json!({"prompt": "a", "model": "claude-opus-4-8"}),
            json!({"prompt": "b", "model": "claude-opus-4-8"}),
        ];
        assert!(!is_heterogeneous(&same));

        // All inherit the parent (no `model`) → homogeneous.
        let inherit = vec![json!({"prompt": "a"}), json!({"prompt": "b"})];
        assert!(!is_heterogeneous(&inherit));

        // One explicit + one inherited are two distinct buckets → heterogeneous.
        let mixed_inherit = vec![
            json!({"prompt": "a", "model": "gpt-5.5-fast"}),
            json!({"prompt": "b"}),
        ];
        assert!(is_heterogeneous(&mixed_inherit));
    }

    #[test]
    fn heterogeneous_counts_smart_routed_models_not_just_the_model_field() {
        // A member routed by `/smart` keeps `model` empty and carries the resolved
        // model under the host smuggle key; heterogeneity must reflect what RUNS,
        // not the (empty) `model` field — else a cross-provider routed fan-out is
        // mis-reported as homogeneous.
        let with_route = |prompt: &str, route_model: &str| -> Value {
            let mut agent = json!({ "prompt": prompt });
            agent.as_object_mut().unwrap().insert(
                crate::misc_tools::smart_router::ROUTE_MODEL_SMUGGLE_KEY.to_string(),
                json!(route_model),
            );
            agent
        };

        // Two distinct routed models (empty `model`) → heterogeneous.
        assert!(
            is_heterogeneous(&[
                with_route("verify", "gpt-5.5-fast"),
                with_route("code", "claude-opus-4-8"),
            ]),
            "distinct smart-route models are heterogeneous even with an empty `model`"
        );
        // The same routed model on both → homogeneous.
        assert!(!is_heterogeneous(&[
            with_route("a", "gpt-5.5-fast"),
            with_route("b", "gpt-5.5-fast"),
        ]));
        // A routed member + a parent-inheriting member → two distinct buckets.
        assert!(is_heterogeneous(&[with_route("a", "gpt-5.5-fast"), json!({"prompt": "b"})]));
        // An explicit on-wire `model` wins over the smuggled route model, so two
        // members with the same explicit model stay homogeneous.
        let mut explicit_over_route = with_route("a", "gpt-5.5-fast");
        explicit_over_route
            .as_object_mut()
            .unwrap()
            .insert("model".to_string(), json!("claude-opus-4-8"));
        assert!(
            !is_heterogeneous(&[
                explicit_over_route,
                json!({"prompt": "b", "model": "claude-opus-4-8"}),
            ]),
            "an explicit on-wire model wins over the smuggled route model"
        );
    }
}
