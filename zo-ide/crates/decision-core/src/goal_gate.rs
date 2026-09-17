//! Goal-completion gate: decide whether a `/goal` automation turn may *stop*.
//!
//! This is the anti-"optimistic stop" brain. A long-horizon goal loop must
//! never declare success on a turn it could not actually verify — the failure
//! mode where an agent says "done" while the work is incomplete. The gate
//! folds the two independent signals a turn can produce into one verdict:
//!
//! - `deterministic` — the typed validators (cargo/git/grep) the user attached
//!   to the goal. `Some(true)` = all green, `Some(false)` = at least one red,
//!   `None` = no deterministic validator was configured (nothing objective to
//!   check).
//! - `semantic` — an *independent* adversarial verifier's verdict on the
//!   turn's change (the deep-lane VERIFY phase). `Some(true)` = accepted,
//!   `Some(false)` = rejected, `None` = no semantic verdict was produced this
//!   turn (e.g. a turn that changed nothing, or a path with no deep gate).
//!
//! The rule is deliberately conservative: completion is only ever `Satisfied`
//! on a *positive* signal, never on the mere absence of a negative one. When
//! neither signal exists the goal is [`GoalCompletion::Unverifiable`] — the
//! caller keeps working within its turn budget and then reports the honest
//! "could not verify" outcome instead of a false success.
//!
//! Pure and total (no IO, every input combination covered), so it is unit
//! tested in isolation and shared verbatim by the live goal controller.

/// The verdict for one goal turn: may the loop stop, and if so, how honestly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GoalCompletion {
    /// A positive verification signal was produced — the goal may stop and be
    /// recorded as genuinely succeeded.
    Satisfied,
    /// A signal was produced but it was negative (a validator is red, or the
    /// verifier rejected the change) — keep working within the turn budget.
    Continue,
    /// No signal at all was available (no deterministic validators *and* no
    /// semantic verdict). The goal cannot be objectively confirmed as
    /// configured; the loop must not claim success. The caller keeps working
    /// within budget and, at the cap, reports an honest "unverified" outcome.
    Unverifiable,
}

/// Fold a turn's deterministic and semantic signals into a stop verdict.
///
/// The rule, stated as intent rather than a raw truth table:
/// - With an objective check present (`deterministic = Some(_)`): the goal is
///   `Satisfied` when the check is green *and* the semantic verifier did not
///   veto it (`semantic != Some(false)`); a red check never stops.
/// - With no objective check (`deterministic = None`): the goal is `Satisfied`
///   only on an explicit semantic accept (`semantic == Some(true)`).
/// - When neither signal exists at all (`None`/`None`) the goal is
///   `Unverifiable` — there is nothing to confirm completion, so the loop must
///   not claim success.
/// - Every other case is `Continue` (keep working within the turn budget).
#[must_use]
pub fn decide_goal_completion(
    deterministic: Option<bool>,
    semantic: Option<bool>,
) -> GoalCompletion {
    let satisfied = match deterministic {
        // Objective check ran: green stops unless the verifier explicitly vetoes.
        Some(true) => semantic != Some(false),
        // Objective check is red: never stop on it.
        Some(false) => false,
        // No objective check: require an explicit semantic accept.
        None => semantic == Some(true),
    };

    if satisfied {
        GoalCompletion::Satisfied
    } else if deterministic.is_none() && semantic.is_none() {
        // No signal of any kind — honestly unverifiable, never a silent success.
        GoalCompletion::Unverifiable
    } else {
        GoalCompletion::Continue
    }
}

#[cfg(test)]
mod tests {
    use super::{decide_goal_completion, GoalCompletion};

    // Full 3×3 truth table: deterministic ∈ {Some(true), Some(false), None} ×
    // semantic ∈ {Some(true), Some(false), None}. Every row is pinned so a
    // future change to the rule cannot silently re-introduce an optimistic stop.

    #[test]
    fn deterministic_green_is_satisfied_unless_verifier_rejects() {
        assert_eq!(
            decide_goal_completion(Some(true), Some(true)),
            GoalCompletion::Satisfied
        );
        assert_eq!(
            decide_goal_completion(Some(true), None),
            GoalCompletion::Satisfied
        );
        // Tests pass but the adversarial verifier rejected the change: do NOT stop.
        assert_eq!(
            decide_goal_completion(Some(true), Some(false)),
            GoalCompletion::Continue
        );
    }

    #[test]
    fn deterministic_red_always_continues() {
        assert_eq!(
            decide_goal_completion(Some(false), Some(true)),
            GoalCompletion::Continue
        );
        assert_eq!(
            decide_goal_completion(Some(false), Some(false)),
            GoalCompletion::Continue
        );
        assert_eq!(
            decide_goal_completion(Some(false), None),
            GoalCompletion::Continue
        );
    }

    #[test]
    fn no_validators_defers_to_semantic_verdict() {
        assert_eq!(
            decide_goal_completion(None, Some(true)),
            GoalCompletion::Satisfied
        );
        assert_eq!(
            decide_goal_completion(None, Some(false)),
            GoalCompletion::Continue
        );
    }

    #[test]
    fn no_signal_at_all_is_unverifiable_never_satisfied() {
        // The core anti-optimistic-stop guarantee: absence of a negative is
        // never treated as success.
        assert_eq!(
            decide_goal_completion(None, None),
            GoalCompletion::Unverifiable
        );
    }
}
