//! The three figures at the head of the stats pane.
//!
//! Orca's `StatsPane` opens with `Agents spawned`, `Time agents worked` and
//! `PRs created`, under a line saying since when it has been counting
//! (`StatsPane.tsx:41-53,111-150`). Those are not derivable from any ledger
//! this window already reads: a token ledger knows what a model was asked, not
//! that a person started an agent, and a git history knows a branch was pushed,
//! not that this window opened the pull request. So they are COUNTED, and this
//! is the counting rule.
//!
//! ## Why counters rather than an event log
//!
//! An event log would answer questions nobody asks here — the pane shows three
//! totals and one date. A log of every agent this machine ever started would
//! grow without bound to answer them, and would have to be read in full on
//! every open. Four numbers are the whole product, so four numbers are what is
//! kept.
//!
//! ## What is NOT counted
//!
//! Time an agent is still working. A pane that has been open for three hours
//! has not yet contributed three hours: the figure moves when the session ends,
//! because a total that changes while nobody did anything reads as a clock
//! rather than as a record. The pane draws the same number twice in a row when
//! nothing finished, which is the honest answer.

use serde::{Deserialize, Serialize};

/// The counters, as they are kept and as the pane reads them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct StatsCounters {
    /// When counting began — the stamp of the first event ever recorded.
    ///
    /// `None` means nothing has happened yet, which the pane says with its
    /// own empty rather than with a date.
    pub first_event_at_ms: Option<i64>,
    pub agents_spawned: i64,
    /// Milliseconds of FINISHED agent sessions. See the module note.
    pub agent_time_ms: i64,
    pub prs_created: i64,
}

impl StatsCounters {
    /// A person started an agent.
    pub fn agent_spawned(&mut self, at_ms: i64) {
        self.agents_spawned += 1;
        self.began(at_ms);
    }

    /// An agent session ended after `ms` milliseconds.
    ///
    /// A negative or absurd span is dropped rather than clamped to zero: a
    /// clock that moved backwards, or a start stamp from a previous boot, is
    /// not a session that lasted no time — it is a measurement this counter
    /// has no business adding.
    pub fn agent_worked(&mut self, ms: i64, at_ms: i64) {
        if ms <= 0 || ms > MAX_SESSION_MS {
            return;
        }
        self.agent_time_ms += ms;
        self.began(at_ms);
    }

    /// This window opened a pull request.
    pub fn pr_created(&mut self, at_ms: i64) {
        self.prs_created += 1;
        self.began(at_ms);
    }

    /// Nothing has been counted yet — the pane's own empty state.
    #[must_use]
    pub fn is_untouched(&self) -> bool {
        self.agents_spawned == 0 && self.prs_created == 0
    }

    /// The earliest stamp wins, so a record repaired out of order does not
    /// move the "counting since" line forward.
    fn began(&mut self, at_ms: i64) {
        if at_ms <= 0 {
            return;
        }
        self.first_event_at_ms = Some(match self.first_event_at_ms {
            Some(held) => held.min(at_ms),
            None => at_ms,
        });
    }
}

/// A day and a bit. A "session" longer than this is a machine that slept with
/// an agent open, or a stamp from before a reboot — either way it would swamp
/// every real session on the pane.
const MAX_SESSION_MS: i64 = 30 * 60 * 60 * 1000;

#[cfg(test)]
mod tests {
    use super::*;

    /// The three verbs each move their own number and nothing else.
    #[test]
    fn each_verb_moves_its_own_figure() {
        let mut held = StatsCounters::default();
        held.agent_spawned(1_000);
        held.agent_spawned(2_000);
        held.pr_created(3_000);
        held.agent_worked(60_000, 4_000);
        assert_eq!(held.agents_spawned, 2);
        assert_eq!(held.prs_created, 1);
        assert_eq!(held.agent_time_ms, 60_000);
    }

    /// The line says since WHEN, so the earliest stamp owns it.
    #[test]
    fn counting_began_at_the_earliest_stamp() {
        let mut held = StatsCounters::default();
        held.agent_spawned(5_000);
        held.pr_created(9_000);
        assert_eq!(held.first_event_at_ms, Some(5_000));
        // A record repaired out of order arrives with an OLDER stamp; the line
        // must move back to it rather than keep the newer one.
        held.agent_spawned(2_000);
        assert_eq!(held.first_event_at_ms, Some(2_000));
    }

    /// A span that cannot be a session is not counted as one.
    #[test]
    fn an_impossible_span_is_dropped_rather_than_clamped() {
        let mut held = StatsCounters::default();
        held.agent_worked(-5, 1_000);
        held.agent_worked(0, 1_000);
        held.agent_worked(MAX_SESSION_MS + 1, 1_000);
        assert_eq!(held.agent_time_ms, 0, "an impossible span was added");
        assert_eq!(
            held.first_event_at_ms, None,
            "a dropped measurement started the clock anyway"
        );
        held.agent_worked(MAX_SESSION_MS, 1_000);
        assert_eq!(held.agent_time_ms, MAX_SESSION_MS, "the bound is inclusive");
    }

    /// The empty state is about what a PERSON did, not about elapsed time.
    #[test]
    fn time_alone_is_not_something_to_show() {
        let mut held = StatsCounters::default();
        assert!(held.is_untouched());
        // Time can accrue from a session that started before this document
        // existed; on its own it is not a reason to draw three cards.
        held.agent_worked(60_000, 1_000);
        assert!(held.is_untouched());
        held.agent_spawned(2_000);
        assert!(!held.is_untouched());
    }

    /// A stamp of zero is a missing clock, not the epoch.
    #[test]
    fn a_missing_stamp_does_not_start_the_counting() {
        let mut held = StatsCounters::default();
        held.agent_spawned(0);
        assert_eq!(held.agents_spawned, 1);
        assert_eq!(held.first_event_at_ms, None);
    }
}
