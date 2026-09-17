//! Strategy pivots: when a loop stalls, try a *different approach* before
//! giving up.
//!
//! The stall detector ([`ProgressTracker`](crate::loop_progress::ProgressTracker))
//! stops a loop that repeats the same failure — but its only outcome is
//! "Failed". That is premature for a genuinely hard goal: the repeat proves
//! the *current approach* is exhausted, not the goal. And the verification
//! ledger's no-net-progress verdict
//! ([`DivergingReason::NoNetProgress`](crate::verify_convergence::DivergingReason))
//! proves more of the *same* verification cannot converge — again a statement
//! about the approach.
//!
//! This ledger arms a small budget of pivot turns: on a stall the caller may
//! spend one to force a re-approach (the pivot prompt forbids re-running the
//! failed approach and demands alternatives in a different means class) instead
//! of terminating. Once the budget is spent, the loop gives up exactly as
//! before — the ledger only ever *delays* the existing honest failure, never
//! prevents it, and never touches success paths. Pure and serializable.

use serde::{Deserialize, Serialize};

/// Default pivot turns a goal may spend across its lifetime. Two: the first
/// pivot re-approaches, the second re-approaches once more with that hindsight;
/// a third stall on a third approach is strong evidence the goal (not the
/// approach) is the problem.
pub const GOAL_PIVOT_BUDGET: u32 = 2;

/// How the caller should react to a stalled (or non-converging) turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StallResponse {
    /// Budget remains: spend one pivot — issue a re-approach turn instead of
    /// terminating. `pivots_left` is the budget remaining AFTER this one.
    Pivot { pivots_left: u32 },
    /// Budget spent: terminate exactly as the pre-pivot behavior did.
    GiveUp,
}

/// Per-run pivot budget tracker. Serializable like the other per-goal ledgers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct PivotLedger {
    pivots_used: u32,
}

impl PivotLedger {
    /// Fold one stalled turn: consume a pivot if any remain. `budget = 0`
    /// disables pivoting entirely (immediate `GiveUp`, the pre-pivot behavior).
    pub fn respond_to_stall(&mut self, budget: u32) -> StallResponse {
        if self.pivots_used >= budget {
            return StallResponse::GiveUp;
        }
        self.pivots_used = self.pivots_used.saturating_add(1);
        StallResponse::Pivot {
            pivots_left: budget - self.pivots_used,
        }
    }

    /// Pivots consumed so far (for reports/tests).
    #[must_use]
    pub const fn pivots_used(&self) -> u32 {
        self.pivots_used
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pivot_ladder_spends_the_budget_then_gives_up() {
        let mut ledger = PivotLedger::default();
        assert_eq!(
            ledger.respond_to_stall(GOAL_PIVOT_BUDGET),
            StallResponse::Pivot { pivots_left: 1 }
        );
        assert_eq!(
            ledger.respond_to_stall(GOAL_PIVOT_BUDGET),
            StallResponse::Pivot { pivots_left: 0 }
        );
        // Third stall: budget spent — the pre-pivot honest failure.
        assert_eq!(ledger.respond_to_stall(GOAL_PIVOT_BUDGET), StallResponse::GiveUp);
        assert_eq!(ledger.pivots_used(), 2);
    }

    #[test]
    fn zero_budget_gives_up_immediately() {
        let mut ledger = PivotLedger::default();
        assert_eq!(ledger.respond_to_stall(0), StallResponse::GiveUp);
        assert_eq!(ledger.pivots_used(), 0, "a disabled ledger never spends");
    }

    #[test]
    fn ledger_roundtrips_through_serde() {
        let mut ledger = PivotLedger::default();
        let _ = ledger.respond_to_stall(2);
        let json = serde_json::to_string(&ledger).expect("serialize");
        let back: PivotLedger = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ledger, back);
    }
}
