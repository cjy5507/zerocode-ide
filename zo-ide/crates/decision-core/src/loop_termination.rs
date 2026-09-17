//! Top-level loop stop decision: fold the goal-completion verdict
//! ([`decide_goal_completion`](crate::goal_gate::decide_goal_completion)) with
//! resource-budget exhaustion and stall detection into a single
//! [`LoopTermination`].
//!
//! Precedence: a genuine `Satisfied` completion always wins (never keep working
//! after a real success). Otherwise a stall, then budget exhaustion (which also
//! encodes the turn cap), stop the loop. The caller maps a non-`Satisfied` stop
//! to its own terminal state using the completion verdict — `Unverifiable` ⇒
//! "unverified", any other ⇒ "failed" — exactly generalizing the pre-existing
//! turn-cap behavior to also fire on token budget and stall.

use crate::goal_gate::GoalCompletion;
use crate::loop_budget::BudgetExhaustion;
use crate::loop_progress::{Progress, StallKind};

/// Whether the loop may keep iterating, and if not, why it stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopTermination {
    /// Keep working within budget.
    Continue,
    /// Stop now, for this reason.
    Done(DoneReason),
}

/// Why a loop stopped, in priority order as evaluated by
/// [`decide_loop_termination`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DoneReason {
    /// The completion gate was satisfied — a genuine success.
    Satisfied,
    /// The loop repeated the same failure and is making no progress.
    Stalled(StallKind),
    /// The resource budget (turns or tokens) is exhausted.
    BudgetExhausted(BudgetExhaustion),
}

/// Decide whether a loop may continue.
///
/// `budget` is the result of [`BudgetLedger::exhaustion`](crate::loop_budget::BudgetLedger::exhaustion)
/// (`None` while budget remains; the turn cap is encoded as
/// [`BudgetExhaustion::Turns`]). `progress` is the stall verdict for this turn.
#[must_use]
pub fn decide_loop_termination(
    completion: GoalCompletion,
    budget: Option<BudgetExhaustion>,
    progress: Progress,
) -> LoopTermination {
    // A genuine success stops immediately, regardless of budget/stall.
    if completion == GoalCompletion::Satisfied {
        return LoopTermination::Done(DoneReason::Satisfied);
    }
    // Making no progress: stop before burning the rest of the budget.
    if let Progress::Stalled(kind) = progress {
        return LoopTermination::Done(DoneReason::Stalled(kind));
    }
    // Out of turns or tokens: stop. The caller reports the terminal completion
    // verdict (failed vs unverified) honestly.
    if let Some(exhaustion) = budget {
        return LoopTermination::Done(DoneReason::BudgetExhausted(exhaustion));
    }
    LoopTermination::Continue
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::loop_progress::StallKind;

    const ADVANCING: Progress = Progress::Advancing;
    const STALLED: Progress = Progress::Stalled(StallKind::RepeatedFailure);

    #[test]
    fn satisfied_completion_wins_over_budget_and_stall() {
        assert_eq!(
            decide_loop_termination(
                GoalCompletion::Satisfied,
                Some(BudgetExhaustion::Turns),
                STALLED
            ),
            LoopTermination::Done(DoneReason::Satisfied)
        );
    }

    #[test]
    fn stall_stops_before_budget_when_not_satisfied() {
        assert_eq!(
            decide_loop_termination(GoalCompletion::Continue, None, STALLED),
            LoopTermination::Done(DoneReason::Stalled(StallKind::RepeatedFailure))
        );
    }

    #[test]
    fn budget_exhaustion_stops_an_unsatisfied_loop() {
        assert_eq!(
            decide_loop_termination(
                GoalCompletion::Continue,
                Some(BudgetExhaustion::Tokens),
                ADVANCING
            ),
            LoopTermination::Done(DoneReason::BudgetExhausted(BudgetExhaustion::Tokens))
        );
    }

    #[test]
    fn unsatisfied_with_budget_remaining_continues() {
        assert_eq!(
            decide_loop_termination(GoalCompletion::Continue, None, ADVANCING),
            LoopTermination::Continue
        );
        // Unverifiable with budget left also continues — the anti-optimistic-stop
        // guarantee: it only becomes "unverified" once the budget is spent (the
        // caller maps `BudgetExhausted` + `Unverifiable` → unverified).
        assert_eq!(
            decide_loop_termination(GoalCompletion::Unverifiable, None, ADVANCING),
            LoopTermination::Continue
        );
    }
}
