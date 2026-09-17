//! The last iterations of a finite tool budget ask for the report.
//!
//! An unattended run (a spawned sub-agent, a headless turn, a deep-gate leg)
//! has a finite iteration cap. Before this module, the cap was a wall: the
//! run kept calling tools right up to it and ended `BudgetExhausted` with a
//! harness-written closer — the model never got the message that time was
//! up. Measured on this repository's agent records (2026-09-10): of 26
//! Explore runs, six hit the 64-iteration cap; three of those FAILED with a
//! last turn of 90–322 output tokens ("I'll trace…") after 18–21k output
//! tokens and 150–187 tool calls each — a fifth of every Explore token spent
//! for no report at all.
//!
//! So the last [`WRAP_UP_ITERATIONS`] iterations carry a transient reminder
//! that says how many rounds remain and asks for the final answer now, and
//! the very last iteration forbids tools on the wire (`tool_choice: none`) so
//! the run ends on the model's own text instead of the harness closer. An
//! unbounded interactive turn (`max_iterations == usize::MAX`) never reaches
//! either — a person is the budget there.

use super::ConversationRuntime;
use super::{ApiClient, ToolExecutor};

/// Prefix of the transient wrap-up reminder (replace-by-prefix; cleared at
/// every turn start with the other per-turn reminders).
pub(super) const BUDGET_WRAP_UP_REMINDER_PREFIX: &str = "[zo:budget-wrap-up]";

/// How many of the last iterations carry the wrap-up reminder. Two: one
/// round to finish what is in flight, one to write the answer with tools
/// still allowed, then the final round with tools forbidden.
pub(super) const WRAP_UP_ITERATIONS: usize = 2;

/// Where an iteration stands against the cap.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum WrapUp {
    /// Plenty of budget left, or no cap at all — nothing said.
    Free,
    /// Inside the last [`WRAP_UP_ITERATIONS`]: remind, tools still allowed.
    /// `remaining` is how many iterations follow this one.
    Warn { remaining: usize },
    /// The last iteration: remind and forbid tools, so the run ends on text.
    Final,
}

/// Judge iteration `iterations` (1-based, already incremented) against the cap.
#[must_use]
pub(super) fn wrap_up_for(iterations: usize, max_iterations: usize) -> WrapUp {
    if max_iterations == usize::MAX || iterations > max_iterations {
        return WrapUp::Free;
    }
    let remaining = max_iterations - iterations;
    if remaining == 0 {
        WrapUp::Final
    } else if remaining < WRAP_UP_ITERATIONS {
        WrapUp::Warn { remaining }
    } else {
        WrapUp::Free
    }
}

/// The reminder text for a stage that speaks.
#[must_use]
pub(super) fn budget_wrap_up_reminder(stage: WrapUp) -> Option<String> {
    match stage {
        WrapUp::Free => None,
        WrapUp::Warn { remaining } => Some(format!(
            "{BUDGET_WRAP_UP_REMINDER_PREFIX} <system-reminder>This run's tool budget ends after \
             {remaining} more round{} of tool calls. Stop exploring and write your final answer \
             now, from what you already have: the findings, the exact file references, and what \
             remains unverified. Any tool call you still make must be the one that lets you \
             answer.</system-reminder>",
            if remaining == 1 { "" } else { "s" }
        )),
        WrapUp::Final => Some(format!(
            "{BUDGET_WRAP_UP_REMINDER_PREFIX} <system-reminder>This is the last round of this \
             run's tool budget and no tool call will be executed. Write your final answer now: \
             the findings and exact file references you have, and what you could not verify. Do \
             not describe what you would do next.</system-reminder>"
        )),
    }
}

impl<C, T> ConversationRuntime<C, T>
where
    C: ApiClient,
    T: ToolExecutor,
{
    /// Install (or clear) the wrap-up reminder for this iteration and say what
    /// the request's `tool_choice` must be: `None` while tools are allowed,
    /// `Some(ToolChoice::None)` on the final iteration.
    pub(super) fn arm_budget_wrap_up(&mut self, iterations: usize) -> Option<::api::ToolChoice> {
        let stage = wrap_up_for(iterations, self.max_iterations);
        let reminder = budget_wrap_up_reminder(stage);
        self.replace_transient_system_reminder_by_prefix(
            BUDGET_WRAP_UP_REMINDER_PREFIX,
            reminder.as_deref(),
        );
        matches!(stage, WrapUp::Final).then_some(::api::ToolChoice::None)
    }
}

#[cfg(test)]
mod tests {
    use super::{budget_wrap_up_reminder, wrap_up_for, WrapUp, WRAP_UP_ITERATIONS};

    #[test]
    fn the_stage_follows_the_distance_to_the_cap() {
        assert_eq!(wrap_up_for(1, usize::MAX), WrapUp::Free);
        assert_eq!(wrap_up_for(1, 4), WrapUp::Free);
        assert_eq!(wrap_up_for(2, 4), WrapUp::Free);
        assert_eq!(wrap_up_for(3, 4), WrapUp::Warn { remaining: 1 });
        assert_eq!(wrap_up_for(4, 4), WrapUp::Final);
        // Past the cap the loop has already ended the turn; nothing to say.
        assert_eq!(wrap_up_for(5, 4), WrapUp::Free);
        assert_eq!(WRAP_UP_ITERATIONS, 2);
    }

    #[test]
    fn only_the_speaking_stages_carry_a_reminder() {
        assert_eq!(budget_wrap_up_reminder(WrapUp::Free), None);
        let warn = budget_wrap_up_reminder(WrapUp::Warn { remaining: 1 }).expect("warn text");
        assert!(warn.starts_with("[zo:budget-wrap-up]") && warn.contains("1 more round of tool calls"));
        let final_ = budget_wrap_up_reminder(WrapUp::Final).expect("final text");
        assert!(final_.contains("no tool call will be executed"));
    }
}
