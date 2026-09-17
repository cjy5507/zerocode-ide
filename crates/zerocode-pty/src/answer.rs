//! Whether a child has said anything since it was written to.
//!
//! A person types, and what they are waiting for is the child's answer — the
//! echo of the key, the redraw of a composer line. The pump in the window
//! chases that answer with brisk looks, and it can only stop chasing, or tell
//! a screen that is flowing anyway, once it knows the answer is in. "Something
//! was parsed this round" does not say that: output that was already on its
//! way before the write parses in the same round as anything after it, and
//! another shell's output has nothing to do with this one's write at all.
//!
//! So the question is asked of time, on the only clocks that can answer it:
//! when each write went to the child, and when each chunk of output ARRIVED
//! from it — stamped by the thread that read it, not by whoever parses it
//! later. The first chunk to arrive after a write is that write's answer. It
//! may still be output that merely happened to arrive then; nothing on this
//! side of the pty can tell an echo from a coincidence, and the window never
//! needs it to — a wrong guess costs a frame, as not guessing did.

use std::time::Instant;

/// The writes a child has not yet answered, by when they went.
///
/// Only a program's own input is counted. The terminal's answers to the
/// program's questions (a cursor report, a colour query) are not a person
/// waiting for anything, and are written around this.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Unanswered {
    /// The earliest write the child has said nothing after.
    since: Option<Instant>,
    /// The latest write, which a chunk that arrived before it cannot answer.
    last: Option<Instant>,
}

impl Unanswered {
    /// Something went to the child at `at`.
    pub fn sent(&mut self, at: Instant) {
        self.since.get_or_insert(at);
        self.last = Some(at);
    }

    /// A chunk that arrived at `at` was parsed. Says whether it is an answer:
    /// the first output to arrive after a write nothing had answered yet.
    ///
    /// It answers every write that went before it arrived. A write that went
    /// after — typing faster than the child echoes — still waits for output
    /// of its own.
    pub fn arrived(&mut self, at: Instant) -> bool {
        match self.since {
            Some(since) if since <= at => {
                self.since = self.last.filter(|last| *last > at);
                true
            }
            _ => false,
        }
    }

    /// The earliest write the child has still said nothing after.
    #[must_use]
    pub const fn since(&self) -> Option<Instant> {
        self.since
    }
}

#[cfg(test)]
mod tests {
    use super::Unanswered;
    use std::time::{Duration, Instant};

    const TICK: Duration = Duration::from_millis(1);

    #[test]
    fn output_already_on_its_way_when_a_person_writes_is_not_the_answer() {
        let before = Instant::now();
        let wrote = before + TICK;
        let mut waiting = Unanswered::default();
        waiting.sent(wrote);
        assert!(
            !waiting.arrived(before),
            "a chunk that arrived before the write was taken for its answer"
        );
        assert_eq!(waiting.since(), Some(wrote), "the write stopped waiting");
    }

    #[test]
    fn the_first_output_after_a_write_answers_it_once() {
        let wrote = Instant::now();
        let mut waiting = Unanswered::default();
        waiting.sent(wrote);
        assert!(waiting.arrived(wrote + TICK), "the answer was not seen");
        assert_eq!(waiting.since(), None, "an answered write is still waiting");
        assert!(
            !waiting.arrived(wrote + TICK * 2),
            "output after the answer answered the same write again"
        );
    }

    #[test]
    fn a_write_that_went_after_the_answer_arrived_waits_for_its_own() {
        let first = Instant::now();
        let answer = first + TICK;
        let second = first + TICK * 2;
        let mut waiting = Unanswered::default();
        waiting.sent(first);
        waiting.sent(second);
        assert!(
            waiting.arrived(answer),
            "the first write's answer was not seen"
        );
        assert_eq!(
            waiting.since(),
            Some(second),
            "a write that went after the answer arrived was counted as answered"
        );
        assert!(
            waiting.arrived(second + TICK),
            "the second write's answer was not seen"
        );
        assert_eq!(waiting.since(), None);
    }

    #[test]
    fn nothing_written_waits_for_nothing() {
        let mut waiting = Unanswered::default();
        assert!(
            !waiting.arrived(Instant::now()),
            "output answered a write nobody made"
        );
        assert_eq!(waiting.since(), None);
    }
}
