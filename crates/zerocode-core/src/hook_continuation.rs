//! Which providers let a hook CONTINUE a turn, and at which moment.
//!
//! An `additionalContext` reply speaks to a turn that is about to begin. This
//! module is about the other end: the moment a turn is ENDING, when a hook may
//! answer with a decision that hands the model one more instruction instead of
//! letting it stop. That is the only road by which a running agent can be told
//! something without a keystroke, a composer draft, or a person pressing
//! Enter.
//!
//! Deliberately its own module rather than another `AgentSpec` field. The spec
//! table is one array of thirty-five entries that several slices edit at once;
//! a capability measured from one vendor's CLI does not need to move
//! thirty-four unrelated rows to be written down.
//!
//! **Measured, not assumed.** Claude Code 2.1.261 was read directly: its
//! `Stop` handler honours `{"decision":"block","reason":…}` (the shipped
//! strings include `Stop hook feedback`, `Stop hook denied continuation).`
//! and `Stop hook block discarded (turn ended by …)`), and its `Stop` payload
//! carries `stop_hook_active` so a hook can refuse to block a turn that is
//! only running because a hook already blocked it. Nothing else in this
//! catalog has been measured to do the same, so nothing else claims it.

use crate::agent::AgentKind;

/// The field a provider's `Stop` payload uses to say "this turn is already a
/// hook's continuation". Answering `block` again on such a turn is how a hook
/// turns into a loop, so the one guard is named here beside the capability.
pub const STOP_HOOK_ACTIVE_FIELD: &str = "stop_hook_active";

/// Where a provider takes non-error context in the middle of a turn.
///
/// A turn end is not the only door. A tool boundary is the other one, and it
/// is the one that matters for an agent in a long implementation: `Stop` alone
/// leaves it deaf until the whole turn is over, which is exactly the delay a
/// person watching this system reported.
///
/// **Measured on the installed Claude 2.1.261.** The binary both emits and
/// consumes `hookSpecificOutput: {hookEventName: "PostToolUse",
/// additionalContext: …}`, and describes that field in its own words as
/// "non-error feedback delivered to the model; the conversation continues so
/// the model can act on it". It also says the injection is read "once for the
/// whole batch", which is why a mailbox that hands its pointer over once is
/// the right shape even though "PostToolUse fires per-tool and may run
/// concurrently for parallel tool calls".
///
/// This reply carries context and NOTHING else: no decision, no
/// `permissionDecision`, no `updatedToolOutput`. A pointer must never be able
/// to change what a tool did or what a person was asked.
///
/// `additionalContext` is not a weaker channel than a decision. The vendor's
/// documentation is explicit that it also CONTINUES the turn — the model acts
/// on what it is given rather than merely reading it — and this module says
/// so here because the opposite is an easy and wrong assumption to make from
/// the field's name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ToolBoundaryContext {
    /// The event whose reply carries the context.
    pub event: &'static str,
}

/// Whether this provider accepts context at a tool boundary, and on which
/// event. `None` for every provider whose behaviour has not been measured.
#[must_use]
pub const fn tool_boundary_context(agent: AgentKind) -> Option<ToolBoundaryContext> {
    match agent {
        AgentKind::Claude => Some(ToolBoundaryContext {
            event: "PostToolUse",
        }),
        _ => None,
    }
}

/// How a provider's turn-end hook is answered when there is something to say.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StopContinuation {
    /// The event whose reply carries the decision.
    pub event: &'static str,
    /// The value the provider reads as "do not stop yet".
    pub decision: &'static str,
}

/// Whether this provider continues a turn from its stop hook, and how.
///
/// `None` is the honest answer for every provider whose behaviour has not
/// been measured. A caller that gets `None` must say the route is
/// unsupported; it must never report a delivery it did not make.
#[must_use]
pub const fn stop_continuation(agent: AgentKind) -> Option<StopContinuation> {
    match agent {
        AgentKind::Claude => Some(StopContinuation {
            event: "Stop",
            decision: "block",
        }),
        _ => None,
    }
}

/// Whether this `Stop` payload says the turn is already a hook's continuation.
///
/// Read off the payload rather than remembered, because the provider is the
/// one that knows: a window that kept its own flag would answer for a session
/// it had lost track of across a restart. A payload that cannot be parsed is
/// treated as "already continuing" — the safe direction, since the cost of a
/// missed pointer is a beat's delay and the cost of a wrong block is a loop.
#[must_use]
pub fn stop_is_already_a_continuation(payload: &str) -> bool {
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(payload) else {
        return true;
    };
    parsed
        .get(STOP_HOOK_ACTIVE_FIELD)
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_measured_provider_claims_a_stop_continuation() {
        let claude = stop_continuation(AgentKind::Claude).expect("claude was measured");
        assert_eq!(claude.event, "Stop");
        assert_eq!(claude.decision, "block");
        for unmeasured in [
            AgentKind::Codex,
            AgentKind::Zo,
            AgentKind::Cursor,
            AgentKind::Droid,
            AgentKind::Amp,
        ] {
            assert!(
                stop_continuation(unmeasured).is_none(),
                "{unmeasured:?} claimed a capability nobody measured"
            );
        }
    }

    #[test]
    fn only_the_measured_provider_takes_context_at_a_tool_boundary() {
        let claude = tool_boundary_context(AgentKind::Claude).expect("claude was measured");
        assert_eq!(claude.event, "PostToolUse");
        for unmeasured in [AgentKind::Codex, AgentKind::Zo, AgentKind::Droid] {
            assert!(
                tool_boundary_context(unmeasured).is_none(),
                "{unmeasured:?} claimed a capability nobody measured"
            );
        }
        // The two doors are different events, or one of them is answering the
        // other's knock.
        assert_ne!(
            stop_continuation(AgentKind::Claude)
                .expect("a stop road")
                .event,
            claude.event
        );
    }

    #[test]
    fn a_turn_a_hook_already_continued_is_never_blocked_again() {
        assert!(stop_is_already_a_continuation(
            r#"{"stop_hook_active":true}"#
        ));
        assert!(!stop_is_already_a_continuation(
            r#"{"stop_hook_active":false}"#
        ));
        assert!(!stop_is_already_a_continuation(r#"{"session_id":"s-1"}"#));
        // Unreadable is treated as "already continuing": a missed pointer
        // costs a beat, a wrong block costs a loop.
        assert!(stop_is_already_a_continuation("not json at all"));
    }
}
