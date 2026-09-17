//! The autonomous completion loop's decision brain (orchestration-accuracy P2).
//!
//! Pure policy — no I/O, no spawning. Given a finished verification verdict,
//! the current round, and a per-difficulty ceiling, it decides whether the work
//! is done, should retry with the failure carried forward, or has hit its
//! ceiling and must escalate rather than loop forever. The workflow engine's
//! repair loop already follows this shape (fixer -> reverify -> repeat, capped
//! by `max_attempts` with a same-finding stop); extracting the decision here
//! makes the policy testable in isolation and reusable by a headless auto-verify
//! caller, and pins the design's "confess at the ceiling, never silently paint
//! it green" contract.

use super::policy::RouteTaskComplexity;

/// Maximum autonomous repair rounds a verification loop may run before it must
/// escalate, keyed by task complexity. ONE table — the no-hardcoding contract,
/// the same discipline as the host-prelude fan-out width table. Harder tasks
/// earn more rounds; a trivial task gets a single corrective pass. `Unknown`
/// takes the cautious middle rather than the maximum.
#[must_use]
pub fn completion_ceiling_for(complexity: RouteTaskComplexity) -> usize {
    match complexity {
        RouteTaskComplexity::Trivial => 1,
        // Small and Unknown share the cautious 2-round budget.
        RouteTaskComplexity::Small | RouteTaskComplexity::Unknown => 2,
        RouteTaskComplexity::Medium => 3,
        RouteTaskComplexity::Large => 4,
    }
}

/// The completion loop's decision after one verification verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionStep {
    /// The work verified correct — stop, success.
    Done,
    /// Not yet correct and the ceiling is not reached — retry, carrying the
    /// failure forward, at this (1-based) round number.
    Retry { next_round: usize },
    /// Not correct and the ceiling is reached — stop and escalate (confess the
    /// last failure) rather than loop forever.
    Escalate,
}

/// Decide the next move after a verification verdict. `round` is the 1-based
/// number of the attempt that just produced `passed`; `ceiling` is the maximum
/// number of attempts (clamped to `>=1`). A pass ends the loop; a failure
/// retries until the ceiling, then escalates — NEVER an unbounded retry, so a
/// loop with a broken task confesses its last failure instead of spinning
/// forever or silently reporting success.
#[must_use]
pub fn completion_loop_step(passed: bool, round: usize, ceiling: usize) -> CompletionStep {
    if passed {
        return CompletionStep::Done;
    }
    let ceiling = ceiling.max(1);
    if round >= ceiling {
        CompletionStep::Escalate
    } else {
        CompletionStep::Retry {
            next_round: round.saturating_add(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pass_ends_the_loop_at_any_round() {
        assert_eq!(completion_loop_step(true, 1, 3), CompletionStep::Done);
        assert_eq!(completion_loop_step(true, 3, 3), CompletionStep::Done);
    }

    #[test]
    fn a_failure_below_the_ceiling_retries_carrying_forward() {
        assert_eq!(
            completion_loop_step(false, 1, 3),
            CompletionStep::Retry { next_round: 2 }
        );
        assert_eq!(
            completion_loop_step(false, 2, 3),
            CompletionStep::Retry { next_round: 3 }
        );
    }

    #[test]
    fn a_failure_at_the_ceiling_escalates_never_loops_forever() {
        // The whole point of the ceiling: a broken task confesses, it does not
        // spin. round == ceiling and round > ceiling both escalate.
        assert_eq!(completion_loop_step(false, 3, 3), CompletionStep::Escalate);
        assert_eq!(completion_loop_step(false, 5, 3), CompletionStep::Escalate);
        // A single-attempt ceiling never retries.
        assert_eq!(completion_loop_step(false, 1, 1), CompletionStep::Escalate);
        // A zero/garbage ceiling clamps to 1 rather than looping.
        assert_eq!(completion_loop_step(false, 1, 0), CompletionStep::Escalate);
    }

    #[test]
    fn ceiling_table_is_monotonic_and_always_allows_at_least_one_pass() {
        let trivial = completion_ceiling_for(RouteTaskComplexity::Trivial);
        let small = completion_ceiling_for(RouteTaskComplexity::Small);
        let medium = completion_ceiling_for(RouteTaskComplexity::Medium);
        let large = completion_ceiling_for(RouteTaskComplexity::Large);
        let unknown = completion_ceiling_for(RouteTaskComplexity::Unknown);
        assert!(trivial >= 1, "every task earns at least one corrective pass");
        assert!(trivial <= small && small <= medium && medium <= large);
        assert!(unknown >= small && unknown <= large, "unknown takes the cautious middle");
    }
}
