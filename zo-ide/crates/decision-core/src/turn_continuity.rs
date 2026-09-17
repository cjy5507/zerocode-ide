//! Turn-continuity gate: whether a *behavioral* stop heuristic may end the
//! current turn, or must first spend one advisory round.
//!
//! A turn can stop for two very different kinds of reason, and conflating them
//! is what makes an agent loop feel unreliable:
//!
//! - **Resource breakers** — the wall clock, the output/input token budgets, the
//!   iteration cap. These meter *cost*. They are not opinions about the work, so
//!   they are absolute: when one trips, the turn stops.
//! - **Behavior heuristics** — the verification treadmill (too many
//!   plan/validate/spawn rounds with no file change) and the tool-call
//!   repetition guards. These *infer* that the turn is stuck from a proxy
//!   signal, and a proxy can be wrong: an orchestrator that delegates every edit
//!   looks exactly like a treadmill, and a long editing session legitimately
//!   re-runs the same probe. When one of these fires on a turn that has
//!   demonstrably been producing work, ending the turn throws away a productive
//!   run on a guess.
//!
//! This module encodes that distinction. A heuristic signal on a turn with fresh
//! objective progress buys exactly one [`TurnContinuity::Nudge`] — the guard's
//! advisory is delivered, the turn keeps going, and the caller advances its
//! progress watermark so the *next* firing needs genuinely new progress to be
//! spared again. Everything else stops. Pure and total, so the policy is unit
//! tested in isolation and shared verbatim by both turn loops.

/// The reason a turn loop is considering a stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnStopSignal {
    /// Consecutive plan/validate/spawn rounds with no file mutation.
    VerifyTreadmill,
    /// The same tool call repeated within one turn.
    RepetitionPerTurn,
    /// The same tool call repeated across separate turns.
    RepetitionCrossTurn,
    /// The turn's wall-clock budget is spent.
    Deadline,
    /// The turn's cumulative output-token budget is spent.
    OutputTokens,
    /// The turn's cumulative full-price input-token budget is spent.
    InputTokens,
    /// The turn hit its iteration cap.
    Iterations,
}

/// Whether `signal` is a *behavior heuristic* (an inference about whether the
/// turn is stuck) rather than a resource breaker (a cost measurement).
///
/// Only heuristics are ever eligible for a nudge. A new resource breaker
/// therefore defaults to the safe side: it stops the turn unless it is
/// explicitly listed here.
#[must_use]
pub const fn signal_is_heuristic(signal: TurnStopSignal) -> bool {
    matches!(
        signal,
        TurnStopSignal::VerifyTreadmill
            | TurnStopSignal::RepetitionPerTurn
            | TurnStopSignal::RepetitionCrossTurn
    )
}

/// What the turn loop should do about a stop signal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TurnContinuity {
    /// No stop condition applies — keep looping. Returned by the *callers* (a
    /// guard whose threshold was not reached); [`decide_turn_continuity`] itself
    /// is only asked once a signal has already fired.
    Continue,
    /// Deliver the guard's advisory but keep the turn alive: the heuristic fired
    /// on a turn that is demonstrably still producing work.
    Nudge,
    /// End the turn now.
    Stop,
}

/// Decide whether a fired stop signal ends the turn.
///
/// `progress_since_last_signal` must be derived from *objective* progress —
/// successful file mutations / plan transitions, never reads or probes — so a
/// grind that produces plenty of output while converging nowhere cannot buy
/// itself extra rounds. `nudges_spent`/`max_nudges` are turn-scoped and shared
/// across every heuristic, so the total extra work one turn can win is bounded
/// no matter how many different guards fire.
#[must_use]
pub fn decide_turn_continuity(
    signal: TurnStopSignal,
    progress_since_last_signal: bool,
    nudges_spent: u32,
    max_nudges: u32,
) -> TurnContinuity {
    // Cost breakers are never negotiable, however productive the turn has been.
    if !signal_is_heuristic(signal) {
        return TurnContinuity::Stop;
    }
    if progress_since_last_signal && nudges_spent < max_nudges {
        return TurnContinuity::Nudge;
    }
    TurnContinuity::Stop
}

#[cfg(test)]
mod tests {
    use super::*;

    const RESOURCE_SIGNALS: [TurnStopSignal; 4] = [
        TurnStopSignal::Deadline,
        TurnStopSignal::OutputTokens,
        TurnStopSignal::InputTokens,
        TurnStopSignal::Iterations,
    ];
    const HEURISTIC_SIGNALS: [TurnStopSignal; 3] = [
        TurnStopSignal::VerifyTreadmill,
        TurnStopSignal::RepetitionPerTurn,
        TurnStopSignal::RepetitionCrossTurn,
    ];

    #[test]
    fn resource_breakers_are_never_demoted() {
        // The core guarantee: a cost breaker stops the turn even on a turn that
        // has been making progress with its whole nudge budget unspent.
        for signal in RESOURCE_SIGNALS {
            assert!(!signal_is_heuristic(signal), "{signal:?} is a cost breaker");
            assert_eq!(
                decide_turn_continuity(signal, true, 0, 1),
                TurnContinuity::Stop,
                "{signal:?} must stop regardless of progress"
            );
        }
    }

    #[test]
    fn heuristic_with_fresh_progress_nudges_once() {
        for signal in HEURISTIC_SIGNALS {
            assert!(signal_is_heuristic(signal), "{signal:?} is a heuristic");
            assert_eq!(
                decide_turn_continuity(signal, true, 0, 1),
                TurnContinuity::Nudge,
                "{signal:?} on a productive turn buys one round"
            );
            // The budget is spent: the same signal now stops.
            assert_eq!(
                decide_turn_continuity(signal, true, 1, 1),
                TurnContinuity::Stop,
                "{signal:?} must stop once the nudge budget is spent"
            );
        }
    }

    #[test]
    fn heuristic_without_fresh_progress_always_stops() {
        // The no-progress spawn loop / re-read loop these guards exist to catch:
        // no objective progress means no reprieve, even with budget remaining.
        for signal in HEURISTIC_SIGNALS {
            assert_eq!(
                decide_turn_continuity(signal, false, 0, 1),
                TurnContinuity::Stop
            );
        }
    }

    #[test]
    fn zero_nudge_budget_restores_the_unconditional_stop() {
        for signal in HEURISTIC_SIGNALS {
            assert_eq!(
                decide_turn_continuity(signal, true, 0, 0),
                TurnContinuity::Stop,
                "max_nudges = 0 must behave exactly like the pre-nudge guard"
            );
        }
    }
}
