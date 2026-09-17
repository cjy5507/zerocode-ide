//! A lane: one agent, in one pane, working one worktree.
//!
//! The lane is the unit the whole product is organized around — the rail in the
//! sidebar, the row in the switcher, the thing that holds or waits for the
//! harness helm. `LaneState` is what the rail paints, so it is deliberately
//! small: five states a person can tell apart at a glance from 3px of colour.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::agent::AgentKind;
use crate::pane::PaneKey;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LaneId(pub Uuid);

impl LaneId {
    pub fn random() -> Self {
        LaneId(Uuid::new_v4())
    }
}

impl std::fmt::Display for LaneId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LaneState {
    /// Alive, nothing in flight.
    #[default]
    Idle,
    /// A turn is producing output right now.
    Streaming,
    /// Stopped on a permission gate. Nothing moves until a human answers, and
    /// the harness hard-denies on timeout — so this state must be impossible to
    /// miss in the UI.
    AwaitingPermission,
    /// Stopped on something the agent cannot resolve alone (helm held by another
    /// client, rate limit, failed command).
    Blocked,
    /// The agent process ended.
    Exited,
}

impl LaneState {
    /// No further output will arrive without a restart.
    pub const fn is_terminal(self) -> bool {
        matches!(self, LaneState::Exited)
    }

    /// The lane needs a human. Drives rail emphasis and the dock badge count.
    pub const fn needs_attention(self) -> bool {
        matches!(self, LaneState::AwaitingPermission | LaneState::Blocked)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Lane {
    pub id: LaneId,
    pub pane_key: PaneKey,
    pub agent: AgentKind,
    pub state: LaneState,
    /// Title as reported by the running program (OSC 0/2) or derived from the
    /// task; empty until the agent says something.
    #[serde(default)]
    pub title: String,
    /// Worktree this lane is checked out in. `None` means it runs directly in
    /// the project root.
    #[serde(default)]
    pub worktree_id: Option<String>,
    /// The serve session this lane is attached to. Known from birth — the
    /// window creates the session first and attaches by id — so the
    /// structured channel can always be joined to the pty channel. `None`
    /// only for a lane hosting something that is not a session at all.
    #[serde(default)]
    pub session_id: Option<String>,
}

impl Lane {
    pub fn new(agent: AgentKind, pane_key: PaneKey) -> Self {
        Self {
            id: LaneId::random(),
            pane_key,
            agent,
            state: LaneState::Idle,
            title: String::new(),
            worktree_id: None,
            session_id: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attention_states_are_exactly_the_two_that_block_a_human() {
        let attention: Vec<LaneState> = [
            LaneState::Idle,
            LaneState::Streaming,
            LaneState::AwaitingPermission,
            LaneState::Blocked,
            LaneState::Exited,
        ]
        .into_iter()
        .filter(|state| state.needs_attention())
        .collect();
        assert_eq!(
            attention,
            vec![LaneState::AwaitingPermission, LaneState::Blocked]
        );
    }

    #[test]
    fn only_exited_is_terminal() {
        assert!(LaneState::Exited.is_terminal());
        assert!(!LaneState::Blocked.is_terminal());
        assert!(!LaneState::Streaming.is_terminal());
    }

    #[test]
    fn lane_states_serialize_as_snake_case() {
        assert_eq!(
            serde_json::to_string(&LaneState::AwaitingPermission).expect("serialize"),
            "\"awaiting_permission\""
        );
        assert_eq!(LaneState::default(), LaneState::Idle);
    }
}
