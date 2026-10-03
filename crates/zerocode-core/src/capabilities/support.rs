use serde::Serialize;

use super::{SpawnRoad, SubmitAck};
use crate::agent::{AgentKind, AgentSpec, agent_voice};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct AgentSupport {
    pub execution: bool,
    pub structured_observation: bool,
    pub additional_context: bool,
    pub direct_control: bool,
}

impl AgentSupport {
    #[must_use]
    pub fn for_agent(spec: &AgentSpec) -> Self {
        let caps = spec.capabilities();
        let channel = caps.spawn == SpawnRoad::SocketPane;
        let wire = agent_voice(spec.id).wire.is_some();
        Self {
            execution: caps.takes_prompt_at_start() || spec.takes_a_paste(),
            structured_observation: caps.submit_ack != SubmitAck::None || wire,
            additional_context: AgentKind::from_slug(spec.id)
                .and_then(AgentKind::hook_additional_context)
                .is_some(),
            direct_control: channel || wire,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{AGENT_SPECS, agent_presence};

    #[test]
    fn support_is_derived_from_the_same_catalog_as_execution() {
        for spec in AGENT_SPECS {
            let support = AgentSupport::for_agent(spec);
            assert_eq!(
                support.direct_control,
                spec.harness.spawn == SpawnRoad::SocketPane || agent_voice(spec.id).wire.is_some(),
                "{}",
                spec.id
            );
            assert_eq!(
                support.additional_context,
                AgentKind::from_slug(spec.id)
                    .and_then(AgentKind::hook_additional_context)
                    .is_some(),
                "{}",
                spec.id
            );
        }
    }

    #[test]
    fn support_does_not_claim_that_an_absent_agent_is_running() {
        let rows = agent_presence(Some(std::ffi::OsStr::new("")), "macos");
        let codex = rows.iter().find(|row| row.id == "codex").unwrap();
        assert!(!codex.installed);
        assert!(codex.support.direct_control);
        assert_eq!(
            serde_json::to_value(codex.support).unwrap(),
            serde_json::json!({
                "execution": true, "structured_observation": true,
                "additional_context": true, "direct_control": true,
            })
        );
        let fixture: Vec<serde_json::Value> =
            serde_json::from_str(include_str!("../../../../fixtures/agent-support.json")).unwrap();
        for expected in fixture {
            let row = rows
                .iter()
                .find(|row| row.id == expected["id"].as_str().unwrap())
                .unwrap();
            assert_eq!(
                serde_json::to_value(row.support).unwrap(),
                expected["support"]
            );
        }
    }
}
