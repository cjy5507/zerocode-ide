//! Per-loop resource budget: a pure ledger that decides when a long-horizon
//! loop (a `/goal` run or a `/loop`) has spent its allowance and must stop.
//!
//! Output-token usage is accumulated as a raw `u64` rather than the runtime's
//! `TokenUsage` type (which is intentionally not `Serialize`), so the ledger is
//! cheap to persist across restarts. Pure and total — unit tested in isolation.

use serde::{Deserialize, Serialize};

/// The configured ceiling for one loop. A `0`/`None` axis means "no limit on
/// that axis"; an all-unset budget never exhausts on its own (a goal's own
/// `max_turns` still bounds it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LoopBudget {
    /// Hard cap on goal turns / loop runs. `0` means "no turn cap from the
    /// budget" (a goal's controller cap still applies separately).
    pub max_turns: u32,
    /// Cap on cumulative assistant output tokens across the whole loop.
    pub max_output_tokens: Option<u64>,
}

/// Running totals charged against a [`LoopBudget`]. Serializable so a resumed
/// loop keeps counting from where it left off.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct BudgetLedger {
    pub turns: u32,
    pub output_tokens: u64,
}

/// Which axis of the budget ran out — the reason a loop stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BudgetExhaustion {
    Turns,
    Tokens,
}

impl BudgetLedger {
    /// Record one completed turn that produced `output_tokens` assistant tokens.
    pub fn charge(&mut self, output_tokens: u64) {
        self.turns = self.turns.saturating_add(1);
        self.output_tokens = self.output_tokens.saturating_add(output_tokens);
    }

    /// Whether the budget is now exhausted, and on which axis. Turns are checked
    /// before tokens so a turn-capped loop reports the more actionable reason.
    #[must_use]
    pub fn exhaustion(&self, budget: &LoopBudget) -> Option<BudgetExhaustion> {
        if budget.max_turns > 0 && self.turns >= budget.max_turns {
            return Some(BudgetExhaustion::Turns);
        }
        if let Some(max) = budget.max_output_tokens {
            if self.output_tokens >= max {
                return Some(BudgetExhaustion::Tokens);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_budget_never_exhausts() {
        let ledger = BudgetLedger {
            turns: 100,
            output_tokens: 1_000_000,
        };
        assert_eq!(ledger.exhaustion(&LoopBudget::default()), None);
    }

    #[test]
    fn turn_cap_exhausts_at_or_above_max() {
        let budget = LoopBudget {
            max_turns: 3,
            max_output_tokens: None,
        };
        let mut ledger = BudgetLedger::default();
        ledger.charge(10);
        ledger.charge(10);
        assert_eq!(ledger.exhaustion(&budget), None, "2/3 turns: not exhausted");
        ledger.charge(10);
        assert_eq!(
            ledger.exhaustion(&budget),
            Some(BudgetExhaustion::Turns),
            "3/3 turns: exhausted"
        );
    }

    #[test]
    fn token_cap_exhausts_at_or_above_max() {
        let budget = LoopBudget {
            max_turns: 0,
            max_output_tokens: Some(1_000),
        };
        let mut ledger = BudgetLedger::default();
        ledger.charge(600);
        assert_eq!(ledger.exhaustion(&budget), None);
        ledger.charge(400);
        assert_eq!(
            ledger.exhaustion(&budget),
            Some(BudgetExhaustion::Tokens),
            "1000/1000 tokens: exhausted"
        );
    }

    #[test]
    fn turns_take_priority_over_tokens() {
        let budget = LoopBudget {
            max_turns: 1,
            max_output_tokens: Some(1),
        };
        let mut ledger = BudgetLedger::default();
        ledger.charge(1_000);
        assert_eq!(ledger.exhaustion(&budget), Some(BudgetExhaustion::Turns));
    }

    #[test]
    fn ledger_roundtrips_through_serde() {
        let ledger = BudgetLedger {
            turns: 2,
            output_tokens: 1234,
        };
        let json = serde_json::to_string(&ledger).expect("serialize");
        let back: BudgetLedger = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(ledger, back);
    }
}
