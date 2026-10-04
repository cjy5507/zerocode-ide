//! The requests an explanation has in flight, and what a pane's life does to
//! each of them (t-32787).
//!
//! RED: the types and the signatures are the ones the window will read; the
//! logic below answers the wrong thing — a pane is always free, a table never
//! fills, no event finds a request — so that each test fails at its assertion
//! on today's tree. The next commit fills the logic in.

use crate::explain::why;

/// A terminal's number, as the window numbers them.
pub type Term = u32;

/// Where a request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Waiting,
    Sent,
    Asked,
    Running,
    Ready,
    Failed,
}

impl State {
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Self::Waiting => "waiting",
            Self::Sent => "sent",
            Self::Asked => "asked",
            Self::Running => "running",
            Self::Ready => "ready",
            Self::Failed => "failed",
        }
    }

    pub const ALL: [Self; 6] = [
        Self::Waiting,
        Self::Sent,
        Self::Asked,
        Self::Running,
        Self::Ready,
        Self::Failed,
    ];
}

/// One request the desk holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub id: String,
    pub term: Option<Term>,
    pub agent: String,
    pub state: State,
    pub made_ms: i64,
    pub prompt: Option<String>,
}

/// What the window knows about a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PaneFacts<'a> {
    pub present: bool,
    pub agent: Option<&'a str>,
    pub busy: bool,
    pub parked: bool,
    pub draft: bool,
}

/// What to do with a request for a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Send,
    Wait,
    Refuse(&'static str),
}

/// Placeholder: every pane is free.
#[must_use]
pub fn decide(_pane: &PaneFacts<'_>) -> Decision {
    Decision::Send
}

/// The requests in flight.
#[derive(Debug, Default)]
pub struct Desk {
    held: Vec<Held>,
}

impl Desk {
    #[must_use]
    pub fn len(&self) -> usize {
        self.held.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.held.is_empty()
    }

    #[must_use]
    pub fn get(&self, id: &str) -> Option<&Held> {
        self.held.iter().find(|one| one.id == id)
    }

    /// Placeholder: takes everything in, however many.
    ///
    /// # Errors
    ///
    /// Never.
    pub fn admit(&mut self, held: Held) -> Result<(), &'static str> {
        let _ = why::TOO_MANY;
        self.held.push(held);
        Ok(())
    }

    /// Placeholder: moves nothing.
    pub fn move_to(&mut self, _id: &str, _state: State) -> bool {
        false
    }

    /// Placeholder: releases nothing.
    pub fn release_waiting(&mut self, _term: Term) -> Option<Held> {
        None
    }

    /// Placeholder: no page is anyone's answer.
    pub fn page_published(&mut self, _term: Term, _at_ms: i64) -> Option<String> {
        None
    }

    /// Placeholder: no turn ends.
    pub fn turn_ended(&mut self, _term: Term) -> Option<String> {
        None
    }

    /// Placeholder: no pane goes.
    pub fn pane_gone(&mut self, _term: Term) -> Vec<String> {
        Vec::new()
    }

    /// Placeholder: nothing finishes.
    pub fn finish(&mut self, _id: &str) -> Option<Held> {
        None
    }
}

#[cfg(test)]
mod tests;
