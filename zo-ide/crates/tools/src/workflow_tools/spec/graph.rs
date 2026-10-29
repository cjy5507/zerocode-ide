//! Explicit control flow over existing agent phases, checks, and dreaming.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{invalid, NormalizedPhase};
use crate::ToolError;

const MAX_NODES: usize = 256;
const MAX_STEPS: u32 = 10_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GraphSpec {
    pub entry: String,
    /// Counts every visit, including checks, branches, and repeated nodes.
    pub max_steps: u32,
    pub nodes: Vec<GraphNode>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum GraphNode {
    Agent {
        id: String,
        phase: String,
        next: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        on_error: Option<String>,
    },
    Check {
        id: String,
        command: String,
        on_pass: String,
        on_fail: String,
    },
    Branch {
        id: String,
        source: String,
        pointer: String,
        equals: Value,
        on_match: String,
        on_miss: String,
    },
    Dream {
        id: String,
        next: String,
    },
    End {
        id: String,
        status: GraphEndStatus,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GraphEndStatus {
    Completed,
    Failed,
}

impl GraphNode {
    pub(crate) fn id(&self) -> &str {
        match self {
            Self::Agent { id, .. }
            | Self::Check { id, .. }
            | Self::Branch { id, .. }
            | Self::Dream { id, .. }
            | Self::End { id, .. } => id,
        }
    }

    pub(crate) fn targets(&self) -> Vec<&str> {
        match self {
            Self::Agent { next, on_error, .. } => {
                std::iter::once(next.as_str()).chain(on_error.as_deref()).collect()
            }
            Self::Check { on_pass, on_fail, .. } => vec![on_pass, on_fail],
            Self::Branch { on_match, on_miss, .. } => vec![on_match, on_miss],
            Self::Dream { next, .. } => vec![next],
            Self::End { .. } => Vec::new(),
        }
    }
}

impl GraphSpec {
    pub(super) fn validate(&self, phases: &[NormalizedPhase]) -> Result<(), ToolError> {
        if self.max_steps == 0 || self.max_steps > MAX_STEPS {
            return Err(invalid(format!(
                "graph `max_steps` must be between 1 and {MAX_STEPS}"
            )));
        }
        if self.nodes.is_empty() || self.nodes.len() > MAX_NODES {
            return Err(invalid(format!(
                "graph must declare between 1 and {MAX_NODES} nodes"
            )));
        }
        checked_id(&self.entry, "graph `entry`")?;
        let mut nodes = BTreeMap::new();
        for node in &self.nodes {
            checked_id(node.id(), "graph node `id`")?;
            if nodes.insert(node.id(), node).is_some() {
                return Err(invalid(format!("graph node `id` `{}` is not unique", node.id())));
            }
        }
        if !nodes.contains_key(self.entry.as_str()) {
            return Err(invalid(format!("graph entry `{}` does not name a node", self.entry)));
        }
        for node in &self.nodes {
            for target in node.targets() {
                checked_id(target, "graph edge target")?;
                if !nodes.contains_key(target) {
                    return Err(invalid(format!(
                        "graph node `{}` targets unknown node `{target}`", node.id()
                    )));
                }
            }
            validate_node(node, &nodes, phases)?;
        }

        let reachable = reachable_nodes(&nodes, [self.entry.as_str()], false);
        if let Some(id) = nodes.keys().find(|id| !reachable.contains(**id)) {
            return Err(invalid(format!("graph node `{id}` is unreachable from entry")));
        }
        let ends: Vec<&str> = self.nodes.iter().filter_map(|node| {
            matches!(node, GraphNode::End { .. }).then_some(node.id())
        }).collect();
        if ends.is_empty() {
            return Err(invalid("graph must declare an `end` node"));
        }
        let can_end = reachable_nodes(&nodes, ends, true);
        if let Some(id) = nodes.keys().find(|id| !can_end.contains(**id)) {
            return Err(invalid(format!("graph node `{id}` has no path to an end node")));
        }
        Ok(())
    }
}

fn checked_id(value: &str, field: &str) -> Result<(), ToolError> {
    if value.is_empty() || value.trim() != value {
        return Err(invalid(format!(
            "{field} must be non-empty and have no surrounding whitespace"
        )));
    }
    Ok(())
}

fn validate_node(
    node: &GraphNode,
    nodes: &BTreeMap<&str, &GraphNode>,
    phases: &[NormalizedPhase],
) -> Result<(), ToolError> {
    match node {
        GraphNode::Agent { id, phase, .. } => {
            checked_id(phase, "graph agent `phase`")?;
            if !phases.iter().any(|candidate| candidate.id == *phase) {
                return Err(invalid(format!(
                    "graph node `{id}` references unknown phase `{phase}`"
                )));
            }
        }
        GraphNode::Check { id, command, .. } if command.trim().is_empty() => {
            return Err(invalid(format!("graph check `{id}` needs a non-empty command")));
        }
        GraphNode::Branch { id, source, pointer, .. } => {
            checked_id(source, "graph branch `source`")?;
            if !matches!(nodes.get(source.as_str()), Some(GraphNode::Agent { .. } | GraphNode::Check { .. })) {
                return Err(invalid(format!(
                    "graph branch `{id}` source `{source}` must name an agent or check node"
                )));
            }
            if !valid_pointer(pointer) {
                return Err(invalid(format!(
                    "graph branch `{id}` pointer must be a JSON pointer with valid ~0/~1 escapes"
                )));
            }
        }
        GraphNode::Dream { id, next } => {
            if !matches!(nodes.get(next.as_str()), Some(GraphNode::End { status: GraphEndStatus::Completed, .. })) {
                return Err(invalid(format!("dream node `{id}` must lead directly to a completed end")));
            }
        }
        _ => {}
    }
    Ok(())
}

fn valid_pointer(pointer: &str) -> bool {
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return false;
    }
    let mut chars = pointer.chars();
    while let Some(ch) = chars.next() {
        if ch == '~' && !matches!(chars.next(), Some('0' | '1')) {
            return false;
        }
    }
    true
}

fn reachable_nodes<'a>(
    nodes: &BTreeMap<&'a str, &'a GraphNode>,
    seeds: impl IntoIterator<Item = &'a str>,
    reversed: bool,
) -> BTreeSet<&'a str> {
    let mut pending: Vec<&str> = seeds.into_iter().collect();
    let mut seen = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        if reversed {
            pending.extend(nodes.iter().filter_map(|(source, node)| {
                node.targets().contains(&id).then_some(*source)
            }));
        } else if let Some(node) = nodes.get(id) {
            pending.extend(node.targets());
        }
    }
    seen
}

#[cfg(test)]
mod tests {
    use serde_json::{json, Value};

    use super::super::{NormalizedWorkflow, WorkflowMode, WorkflowSpec};

    fn normalize(value: &Value) -> Result<NormalizedWorkflow, crate::ToolError> {
        WorkflowSpec::from_value(value)?.validate()
    }

    fn cycle() -> Value {
        json!({
            "name": "repair-loop", "mode": "graph",
            "phases": [{"id": "fix", "prompt": "Repair the observed failure"}],
            "graph": {"entry": "verify", "max_steps": 12, "nodes": [
                {"kind": "check", "id": "verify", "command": "cargo test", "on_pass": "dream", "on_fail": "repair"},
                {"kind": "agent", "id": "repair", "phase": "fix", "next": "verify", "on_error": "failed"},
                {"kind": "dream", "id": "dream", "next": "done"},
                {"kind": "end", "id": "done", "status": "completed"},
                {"kind": "end", "id": "failed", "status": "failed"}
            ]}
        })
    }

    fn rejects(value: &Value, reason: &str) {
        let message = normalize(value).expect_err("invalid graph").to_string();
        assert!(message.contains(reason), "expected {reason}: {message}");
    }

    #[test]
    fn bounded_cycle_reuses_agent_phases_and_reaches_both_terminals() {
        let workflow = normalize(&cycle()).expect("a bounded repair loop");
        assert_eq!(workflow.mode, WorkflowMode::Graph);
        let graph = workflow.graph.expect("normalized control flow");
        assert_eq!(graph.entry, "verify");
        assert_eq!(graph.max_steps, 12);
        assert_eq!(graph.nodes[1].targets(), ["verify", "failed"]);
    }

    #[test]
    fn check_only_graph_needs_no_agent_phase() {
        normalize(&json!({
            "name": "gate", "mode": "graph",
            "graph": {"entry": "check", "max_steps": 2, "nodes": [
                {"kind": "check", "id": "check", "command": "true", "on_pass": "end", "on_fail": "end"},
                {"kind": "end", "id": "end", "status": "failed"}
            ]}
        })).expect("no agent node needs no phase");
    }

    #[test]
    fn rejects_missing_or_foreign_graph_configuration() {
        let mut value = cycle();
        value.as_object_mut().unwrap().remove("graph");
        rejects(&value, "requires a `graph`");
        for mode in ["phases", "pipeline"] {
            let mut value = cycle();
            value["mode"] = json!(mode);
            rejects(&value, "requires `mode:");
        }
    }

    #[test]
    fn rejects_missing_zero_and_excessive_step_bounds() {
        let mut value = cycle();
        value["graph"].as_object_mut().unwrap().remove("max_steps");
        rejects(&value, "max_steps");
        for steps in [0, 10_001] {
            value["graph"]["max_steps"] = json!(steps);
            rejects(&value, "max_steps");
        }
    }

    #[test]
    fn rejects_unknown_entry_edges_and_agent_phases() {
        for (pointer, replacement, reason) in [
            ("/graph/entry", "missing", "does not name a node"),
            ("/graph/nodes/0/on_pass", "missing", "unknown node"),
            ("/graph/nodes/1/phase", "missing", "unknown phase"),
            ("/graph/nodes/1/on_error", "missing", "unknown node"),
            ("/graph/nodes/0/id", "repair", "not unique"),
            ("/graph/entry", " verify", "surrounding whitespace"),
            ("/graph/nodes/0/on_pass", "dream ", "surrounding whitespace"),
            ("/graph/nodes/1/phase", " fix", "surrounding whitespace"),
            ("/graph/nodes/0/command", " ", "non-empty command"),
        ] {
            let mut value = cycle();
            *value.pointer_mut(pointer).unwrap() = json!(replacement);
            rejects(&value, reason);
        }
    }

    #[test]
    fn rejects_unreachable_nodes_and_closed_cycles() {
        let mut value = cycle();
        value["graph"]["nodes"].as_array_mut().unwrap().push(json!({
            "kind": "end", "id": "unused", "status": "completed"
        }));
        rejects(&value, "unreachable");

        let mut value = cycle();
        value["graph"]["nodes"][1] = json!({
            "kind": "agent", "id": "repair", "phase": "fix", "next": "repair"
        });
        value["graph"]["nodes"].as_array_mut().unwrap().pop();
        rejects(&value, "no path to an end");

        rejects(&json!({
            "name": "never", "mode": "graph",
            "graph": {"entry": "cycle", "max_steps": 3, "nodes": [
                {"kind": "check", "id": "cycle", "command": "true", "on_pass": "cycle", "on_fail": "cycle"}
            ]}
        }), "end` node");
    }

    #[test]
    fn branch_sources_and_json_pointer_escapes_are_validated() {
        let mut value = cycle();
        value["graph"]["nodes"][2] = json!({
            "kind": "branch", "id": "dream", "source": "verify", "pointer": "/exit_code", "equals": 0,
            "on_match": "done", "on_miss": "failed"
        });
        for pointer in ["", "/", "/a~0b/c~1d", "/nested/0"] {
            value["graph"]["nodes"][2]["pointer"] = json!(pointer);
            normalize(&value).expect("valid JSON pointer");
        }
        for pointer in ["missing-slash", "/dangling~", "/bad~2escape"] {
            value["graph"]["nodes"][2]["pointer"] = json!(pointer);
            rejects(&value, "JSON pointer");
        }
        value["graph"]["nodes"][2]["pointer"] = json!("");
        for source in ["absent", "dream", "done"] {
            value["graph"]["nodes"][2]["source"] = json!(source);
            rejects(&value, "must name an agent or check node");
        }
    }

    #[test]
    fn rejects_hidden_final_agents_and_unknown_fields() {
        for field in ["synthesize", "judge"] {
            let mut value = cycle();
            value[field] = json!({"prompt": "Untracked final agent"});
            rejects(&value, "top-level");
        }
        let mut value = cycle();
        value["graph"]["unknown"] = json!(true);
        rejects(&value, "unknown field");
        let mut value = cycle();
        value["graph"]["nodes"][0]["on_pas"] = json!("dream");
        rejects(&value, "unknown field");
    }

    #[test]
    fn graph_cycles_own_retries_instead_of_nesting_phase_repeat_controllers() {
        let mut value = cycle();
        value["phases"][0]["repeat"] = json!({"max_rounds": 2});
        rejects(&value, "explicit cycles and check nodes");

        let mut value = cycle();
        value["phases"][0]["strategy"] = json!("fix_until_verified");
        value["phases"][0]["fixer"] = json!({"prompt": "Repair"});
        value["phases"][0]["final_check"] = json!({"command": "cargo test"});
        rejects(&value, "explicit cycles and check nodes");
    }

    #[test]
    fn rejects_empty_or_excessively_large_graphs() {
        let mut value = cycle();
        value["graph"]["nodes"] = json!([]);
        rejects(&value, "between 1 and 256 nodes");
        value["graph"]["nodes"] = Value::Array((0..257).map(|index| json!({
            "kind": "end", "id": format!("end-{index}"), "status": "completed"
        })).collect());
        rejects(&value, "between 1 and 256 nodes");
    }
}
