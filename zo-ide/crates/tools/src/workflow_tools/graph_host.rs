//! Production adapters for graph identity and evidence-backed dreaming.

use runtime::PermissionMode;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::ToolError;
use super::spec::{ApplyPolicy, GraphNode, Isolation, NormalizedWorkflow, WorkflowMode};

pub(super) fn run_id(input: &Value, spec: &Value, argument: &Value) -> Result<String, ToolError> {
    let explicit = input.get("runId");
    let resume = input.get("resumeFromRunId");
    if explicit.is_some() && resume.is_some() {
        return Err(ToolError::InvalidInput("use runId or resumeFromRunId, not both".to_string()));
    }
    match explicit.or(resume) {
        Some(Value::String(id)) if !id.is_empty() => Ok(id.clone()),
        Some(_) => Err(ToolError::InvalidInput("graph run id must be a non-empty string".to_string())),
        None => Ok(super::cache::compute_run_id(&authored_spec(spec), argument)),
    }
}

pub(super) fn fingerprint(spec: &Value, input: &Value) -> Result<String, ToolError> {
    let cwd = std::env::current_dir().map_err(|e| ToolError::Execution(e.to_string()))?;
    let identity = serde_json::to_value((authored_spec(spec), input, cwd))
        .map_err(|e| ToolError::Execution(e.to_string()))?;
    let canonical = super::cache::canonical_json(&identity);
    Ok(format!("{:x}", Sha256::digest(canonical.as_bytes())))
}

/// A graph's checks run in its current workspace. Refuse launches whose
/// changes cannot reach that workspace instead of verifying an unrelated tree.
pub(super) fn validate_execution_scope(
    workflow: &NormalizedWorkflow,
    mode: Option<PermissionMode>,
    ledger_available: bool,
) -> Result<(), ToolError> {
    if workflow.mode != WorkflowMode::Graph {
        return Ok(());
    }
    let graph = workflow.graph.as_ref().ok_or_else(|| {
        ToolError::InvalidInput("graph execution requires a graph specification".to_string())
    })?;
    for node in &graph.nodes {
        let GraphNode::Agent { id, phase, .. } = node else { continue; };
        let phase = workflow.phases.iter().find(|item| &item.id == phase)
            .ok_or_else(|| ToolError::InvalidInput(format!("graph node `{id}` references an unknown phase")))?;
        let selected_ledger = ledger_available
            && (matches!(mode, None | Some(PermissionMode::DangerFullAccess))
                || phase.launch.agent.is_some()
                || phase.launch.effort.is_some()
                || phase.launch.worktree.is_some());
        if selected_ledger {
            if phase.launch.worktree != Some(false) || workflow.isolation != Isolation::None {
                return Err(ToolError::InvalidInput(format!(
                    "graph node `{id}` checks run in the current workspace; ledger workers require explicit \
                     `worktree: false` and workflow `isolation: none` inside a dedicated graph worktree \
                     until isolated-worker integration is supported"
                )));
            }
        } else if workflow.isolation == Isolation::Worktree && workflow.apply != ApplyPolicy::Sequential {
            return Err(ToolError::InvalidInput(format!(
                "graph node `{id}` checks run in the current workspace; native worktree isolation \
                 requires `apply: sequential` so checks see the worker's changes"
            )));
        }
    }
    Ok(())
}

/// Ledger completions currently carry no measured token usage. A recurring
/// graph cannot treat those workers as free; fail before launching any of them.
pub(super) fn validate_scheduled_accounting(
    workflow: &NormalizedWorkflow, mode: Option<PermissionMode>, ledger_available: bool,
) -> Result<(), ToolError> {
    let Some(graph) = &workflow.graph else { return Ok(()); };
    for node in &graph.nodes {
        let GraphNode::Agent { phase, .. } = node else { continue; };
        let phase = workflow.phases.iter().find(|candidate| &candidate.id == phase)
            .ok_or_else(|| ToolError::InvalidInput("scheduled graph phase is missing".to_string()))?;
        if ledger_available && (matches!(mode, None | Some(PermissionMode::DangerFullAccess))
            || phase.launch.agent.is_some() || phase.launch.effort.is_some() || phase.launch.worktree.is_some()) {
            return Err(ToolError::InvalidInput("scheduled graph requires measured worker usage; ZeroCode ledger workers do not yet provide token receipts".to_string()));
        }
    }
    Ok(())
}

/// The dispatcher supplies fresh execution metadata on every tool call. Like
/// `WorkflowSpec::from_value`, identity ignores only those reserved top-level
/// fields; nested authored data remains part of the run's identity.
fn authored_spec(spec: &Value) -> Value {
    match spec {
        Value::Object(map) => Value::Object(map.iter()
            .filter(|(key, _)| !key.starts_with("__zo_"))
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect()),
        other => other.clone(),
    }
}

pub(super) fn dream(
    checks: &[String],
    session: &str,
    model: Option<&str>,
    mode: Option<PermissionMode>,
) -> Result<Value, String> {
    if matches!(mode, None | Some(PermissionMode::ReadOnly)) {
        return Err("dreaming needs write permission for project memory".to_string());
    }
    if session.trim().is_empty() || checks.is_empty() {
        return Err("dreaming needs a source session and verified checks".to_string());
    }
    let cwd = std::env::current_dir().map_err(|e| e.to_string())?;
    let config = runtime::ConfigLoader::default_for(&cwd).load().map_err(|e| e.to_string())?;
    if std::env::var("ZO_DREAM").ok().as_deref() == Some("0") || !config.dream_automation_enabled() {
        return Ok(json!({"status":"disabled", "promoted":[]}));
    }
    dream_at(&cwd, checks, session, model)
}

fn dream_at(cwd: &std::path::Path, checks: &[String], session: &str, model: Option<&str>) -> Result<Value, String> {
    // Check the entire batch before writing its first observation. Reusable
    // memory follows the same credential rule as MemoryWrite and SkillDistill.
    let fields: Vec<_> = checks.iter().map(|command| ("check", command.as_str())).collect();
    crate::misc_tools::refuse_kept_credentials(&fields).map_err(|error| error.to_string())?;
    for command in checks {
        let observation = runtime::memory::verified_check_observation(session, Some(command))
            .ok_or_else(|| "empty verification command".to_string())?;
        runtime::memory::record_observation(cwd, &observation).map_err(|e| e.to_string())?;
    }
    let report = runtime::memory::dream_at_cwd(cwd, model).map_err(|e| e.to_string())?;
    Ok(json!({
        "status":"completed", "summary":report.summary_line(),
        "promoted": report.applied.iter().map(|item| item.slug.as_str()).collect::<Vec<_>>(),
        "skipped":report.plan.skipped,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core_types::MemoryRetriever;
    use runtime::memory::MemoryStore;

    struct MemoryEnv([(&'static str, Option<std::ffi::OsString>); 5]);

    impl MemoryEnv {
        fn at(root: &std::path::Path) -> Self {
            let keys = ["HOME", "ZO_HOME", "ZO_CONFIG_HOME", "ZO_STATE_DIR", "ZEROCODE_SECOND_BRAIN"];
            let previous = keys.map(|key| (key, std::env::var_os(key)));
            for key in keys {
                std::env::set_var(key, root);
            }
            Self(previous)
        }
    }

    impl Drop for MemoryEnv {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    #[test]
    fn graph_run_identity_is_explicit_or_stable_and_never_ambiguous() {
        let spec = json!({"name":"test"});
        assert_eq!(run_id(&json!({}), &spec, &Value::Null).unwrap(), run_id(&json!({}), &spec, &Value::Null).unwrap());
        assert_eq!(run_id(&json!({"runId":"nightly-1"}), &spec, &Value::Null).unwrap(), "nightly-1");
        assert!(run_id(&json!({"runId":"a","resumeFromRunId":"b"}), &spec, &Value::Null).is_err());
        assert!(run_id(&json!({"runId":4}), &spec, &Value::Null).is_err());
    }

    #[test]
    fn graph_fingerprint_changes_with_edges_or_input() {
        assert_ne!(fingerprint(&json!({"next":"a"}), &json!(1)).unwrap(), fingerprint(&json!({"next":"b"}), &json!(1)).unwrap());
        assert_ne!(fingerprint(&json!({"next":"a"}), &json!(1)).unwrap(), fingerprint(&json!({"next":"a"}), &json!(2)).unwrap());
    }

    #[test]
    fn reordered_nested_spec_and_input_keep_the_same_graph_identity() {
        let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let first: Value = serde_json::from_str(r#"{"name":"graph","graph":{"entry":"a","nodes":[{"id":"a","kind":"end","status":"completed"}]}}"#).unwrap();
        let reordered: Value = serde_json::from_str(r#"{"graph":{"nodes":[{"status":"completed","kind":"end","id":"a"}],"entry":"a"},"name":"graph"}"#).unwrap();
        let input: Value = serde_json::from_str(r#"{"task":{"a":1,"b":2},"labels":[{"x":3,"y":4}]}"#).unwrap();
        let reordered_input: Value = serde_json::from_str(r#"{"labels":[{"y":4,"x":3}],"task":{"b":2,"a":1}}"#).unwrap();
        assert_eq!(fingerprint(&first, &input).unwrap(), fingerprint(&reordered, &reordered_input).unwrap());
        assert_eq!(run_id(&json!({}), &first, &input).unwrap(), run_id(&json!({}), &reordered, &reordered_input).unwrap());
    }

    fn scope_spec() -> Value {
        json!({
            "name":"scope", "mode":"graph",
            "phases":[{"id":"worker", "prompt":"Perform this task"}],
            "graph":{"entry":"work", "max_steps":2, "nodes":[
                {"kind":"agent", "id":"work", "phase":"worker", "next":"done"},
                {"kind":"end", "id":"done", "status":"completed"}
            ]}
        })
    }

    fn scope(value: &Value, mode: Option<PermissionMode>, ledger: bool) -> Result<(), ToolError> {
        let workflow = super::super::spec::WorkflowSpec::from_value(value)?.validate()?;
        validate_execution_scope(&workflow, mode, ledger)
    }

    #[test]
    fn default_ledger_graph_requires_explicit_shared_workspace_launch() {
        for mode in [None, Some(PermissionMode::DangerFullAccess)] {
            let mut spec = scope_spec();
            let refused = scope(&spec, mode, true).unwrap_err().to_string();
            assert!(refused.contains("worktree: false") && refused.contains("dedicated graph worktree"), "{refused}");
            spec["phases"][0]["worktree"] = json!(false);
            scope(&spec, mode, true).expect("explicit shared workspace is visible to checks");
        }
    }

    #[test]
    fn restrictive_native_graph_defaults_keep_the_existing_execution_scope() {
        for mode in [PermissionMode::WorkspaceWrite, PermissionMode::ReadOnly] {
            scope(&scope_spec(), Some(mode), true).expect("restrictive unpinned launches use native execution");
        }
        scope(&scope_spec(), Some(PermissionMode::DangerFullAccess), false).expect("no ledger means native execution");
    }

    #[test]
    fn isolated_ledger_workers_cannot_verify_the_parent_workspace() {
        let mut spec = scope_spec();
        spec["phases"][0]["worktree"] = json!(true);
        assert!(scope(&spec, Some(PermissionMode::WorkspaceWrite), true).is_err());
        spec["phases"][0]["worktree"] = json!(false);
        spec["isolation"] = json!("worktree");
        spec["apply"] = json!("sequential");
        assert!(scope(&spec, Some(PermissionMode::DangerFullAccess), true).is_err());
    }

    #[test]
    fn native_worktree_graph_requires_merge_back_before_checks() {
        let mut spec = scope_spec();
        spec["isolation"] = json!("worktree");
        let refused = scope(&spec, Some(PermissionMode::WorkspaceWrite), false).unwrap_err().to_string();
        assert!(refused.contains("apply: sequential"), "{refused}");
        spec["apply"] = json!("sequential");
        scope(&spec, Some(PermissionMode::WorkspaceWrite), false).expect("native merge-back exposes the edits");
    }

    #[test]
    fn execution_scope_checks_only_referenced_graph_agents() {
        let mut spec = scope_spec();
        spec["phases"][0]["worktree"] = json!(false);
        spec["phases"].as_array_mut().unwrap().push(json!({"id":"unused", "prompt":"Unused", "worktree":true}));
        scope(&spec, None, true).expect("an unreferenced phase does not launch");
        spec.as_object_mut().unwrap().remove("graph");
        spec["mode"] = json!("phases");
        spec["isolation"] = json!("worktree");
        scope(&spec, None, true).expect("legacy phases retain their existing semantics");
    }

    #[test]
    fn fresh_dispatch_metadata_does_not_restart_the_same_bare_graph() {
        let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let first = json!({"name":"graph", "mode":"graph", "__zo_tool_call_id":"call-1"});
        let second = json!({"name":"graph", "mode":"graph", "__zo_tool_call_id":"call-2", "__zo_route":"new"});
        let clean = json!({"name":"graph", "mode":"graph"});
        assert_eq!(run_id(&first, &first, &Value::Null).unwrap(), run_id(&second, &second, &Value::Null).unwrap());
        assert_eq!(fingerprint(&first, &Value::Null).unwrap(), fingerprint(&second, &Value::Null).unwrap());
        assert_eq!(fingerprint(&clean, &Value::Null).unwrap(), fingerprint(&second, &Value::Null).unwrap());
        assert_ne!(fingerprint(&clean, &json!({"__zo_value":1})).unwrap(), fingerprint(&clean, &json!({"__zo_value":2})).unwrap());
    }

    #[test]
    fn graph_dreaming_requires_write_permission_before_any_io() {
        assert!(dream(&["cargo test".into()], "session", None, Some(PermissionMode::ReadOnly)).is_err());
        assert!(dream(&["cargo test".into()], "session", None, None).is_err());
    }

    #[test]
    fn graph_dreaming_promotes_independent_sessions_once_into_real_memory() {
        let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        let _env = MemoryEnv::at(&home);
        let checks = vec!["cargo test -p acme".to_string()];

        let first = dream_at(&workspace, &checks, "source-one", None).unwrap();
        assert_eq!(first["promoted"], json!([]));
        let replay = dream_at(&workspace, &checks, "source-one", None).unwrap();
        assert_eq!(replay["promoted"], json!([]), "replays are not independent evidence");
        let second = dream_at(&workspace, &checks, "source-two", None).unwrap();
        let promoted = second["promoted"].as_array().unwrap();
        assert_eq!(promoted.len(), 1);
        let slug = promoted[0].as_str().unwrap();
        let memory = runtime::memory::paths::memory_write_dir(&workspace, true);
        assert!(memory.starts_with(&home));
        let body = std::fs::read_to_string(memory.join(format!("{slug}.md"))).unwrap();
        assert!(body.contains(&checks[0]));
        assert_eq!(runtime::memory::classify_memory_body(&body).source, runtime::memory::MemorySource::Dreamer);
        let index = std::fs::read_to_string(memory.join("MEMORY.md")).unwrap();
        assert!(index.contains(&format!("]({slug}.md)")));

        let retriever = runtime::memory::load_lexical_memory_retriever(&workspace, None).unwrap();
        let hits = retriever.recall("cargo test acme verify", 3);
        assert!(hits.iter().any(|hit| hit.entry.slug == slug), "the next session must recall the promoted lesson");

        let third = dream_at(&workspace, &checks, "source-three", None).unwrap();
        assert_eq!(third["promoted"], json!([]));
        assert_eq!(runtime::memory::FsMemoryStore::local_at_cwd(&workspace).existing_slugs(), [slug]);
        assert_eq!(std::fs::read_to_string(memory.join(format!("{slug}.md"))).unwrap(), body);
    }

    #[test]
    fn graph_dreaming_refuses_credentials_before_writing_any_observation() {
        let _lock = crate::tests::env_lock().lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let root = tempfile::tempdir().unwrap();
        let home = root.path().join("home");
        let workspace = root.path().join("workspace");
        std::fs::create_dir_all(&home).unwrap();
        std::fs::create_dir_all(&workspace).unwrap();
        let _env = MemoryEnv::at(&home);
        let refused = dream_at(&workspace, &[
            "cargo test -p acme".to_string(),
            "password=fixture-pass-7731 cargo test".to_string(),
        ], "source-one", None).unwrap_err();
        assert!(refused.contains("credential value"));
        assert!(!refused.contains("fixture-pass-7731"));
        assert_eq!(std::fs::read_dir(&home).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(&workspace).unwrap().count(), 0);
    }
}
